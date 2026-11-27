// Needs you: everything that waits for the person's answer, across every
// agent, chat and pipe, in one place, so nothing waits buried in a conversation
// they don't read. Each is a bubble that opens to the card itself, with its
// real controls, so they answer without opening the conversation.
//
// The app keeps nothing of it: what waits is the backend's (each agent's,
// chat's and pipe's waiting_seqs), and each card is fetched as it is now,
// with LoadEntries for an agent's, LoadChatEntries for a chat's and
// LoadPipeMessages for a pipe's.
import 'dart:async' show Timer, unawaited;
import 'dart:math' as math;

import 'package:flutter/foundation.dart' show defaultTargetPlatform;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'app_access_card.dart' show AppAccessCard, appAccessSummary;
import 'app_grant_card.dart' show AppGrantCard, appGrantSummary;
import 'approve_elsewhere.dart' show approvesElsewhere;
import 'ask_card.dart';
import 'backend.dart';
import 'backend_service.dart'
    show
        BackendServiceRestore,
        backendByHand,
        backendServiceSummary,
        backendServiceTitle;
import 'browser/browser_view.dart'
    show BrowserScope, BrowserSessionCard, browserAssists;
import 'browser/session.dart' show browserHost;
import 'characters/character.dart';
import 'checklist_reply.dart' show showChecklistItem;
import 'chats.dart' show ChatGlyph, chatTitle, openChat;
import 'claktory_identity.dart' show claktoryNameInText;
import 'connectors.dart' show ConnectProposalCard, serviceName;
import 'controls.dart';
import 'find.dart';
import 'forms.dart' show archivedProjectOf, rivetIn;
import 'identity.dart';
import 'keep_awake_card.dart';
import 'lid_closed.dart' show LidClosedStuck, lidClosedStuck;
import 'mobile/drawer.dart' show largeText;
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'move.dart' show BackendMove, MoveConfirmCard, MoveStatus, moveStatusOf;
import 'name_role_line.dart';
import 'needs_you_alerts.dart';
import 'needs_you_entries.dart';
import 'needs_you_focus.dart'
    show focusUrgent, needsYouIsFocusMode, openFocusMode;
import 'needs_you_hold.dart';
import 'needs_you_history.dart' show NeedsYouHistoryList, NeedsYouTabs;
import 'pairing.dart' show AllowDeviceCard, deviceCallingItself;
import 'pairing_card.dart' show DevicePairingCard;
import 'pipe_start.dart' show PipeStartCard, pipeStartSummary;
import 'pane_chrome.dart' show InPane;
import 'people/chat_view.dart' show openDmChat;
import 'pipe_timeline.dart' show pipeItemKey, pipeLiaison;
import 'pipes.dart';
import 'protected_place_item.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'provider_wait_card.dart';
import 'recovery_key.dart' show SaveRecoveryKeyForm;
import 'scope.dart';
import 'selection.dart' show CopyShape, SelectableArea, SelectionPane;
import 'secrets.dart'
    show
        SecretRequestCard,
        UnlockSecretsForm,
        secretOutcome,
        secretRequestSummary;
import 'transcript.dart' show ProposalCard, deviceGlyph;
import 'usage.dart' show UsageRing, formatAge;
import 'visible_codes.dart' show proseCodes;
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

  /// A pipe whose other side is now confirmed as another account's
  /// (PipePeer.identity_change, #471 G7): its messages wait and agents
  /// can't send in it until the person trusts it again, whether or not an
  /// agent asked them to.
  pipeIdentity,

  /// A pipe's message held for the person (Pipe.waiting_seqs).
  pipeHeld,

  /// A message from a pipe's other side that no agent was told of, as the
  /// pipe has no lead (Pipe.unanswered_seqs): the person's to read, until
  /// an agent leads the pipe.
  pipeUnanswered,

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

  /// An agent's browser session waiting for the person to choose a tab on
  /// their Mac, or decline (Workspace.browser_sessions, pending).
  browser,

  /// Messages an agent proposed sending to a friend, waiting for the
  /// person to review them in that chat (Workspace.dm_sends), one item per
  /// agent and conversation.
  dmSend,

  /// Agents and chats waiting on a provider's usage limit or credit
  /// (Workspace.provider_waits, #343 U6), one item per provider: the ways
  /// forward for all of them.
  providerWait,

  /// A device waiting to be allowed on this Claktory
  /// (RemoteAccess.pending_devices): the person types the number its
  /// screen shows, or turns it away (#352 P4).
  device,

  /// Whether to keep this Mac awake while agents work, asked once on a
  /// laptop (Workspace.keep_awake.ask, #352 P1): yes or no, in the card.
  keepAwake,

  /// The backend runs without its background service, started by hand
  /// (Workspace.backend_service BY_HAND, friend bug #24): Restore, in the
  /// card.
  backendService,

  /// The Mac can't sleep with its lid closed although Claktory isn't
  /// keeping it awake (KeepAwake.lid_closed STUCK, #371): what fixes it.
  lidStuck,

  /// A place macOS keeps from Claktory Agents (Documents, a volume…)
  /// refused it, with agents working there (ProtectedPlace.needs_you,
  /// permissions.md P2): Open System Settings, or OK.
  protectedPlace,

  /// An alert an agent (or Claktory) sent, kept until the person reads it
  /// (Workspace.alerts; Lucy's needs-you-cards.md §3.9): nothing to
  /// decide, so a light row of its own (needs_you_alerts.dart).
  alert,

  /// Three or more alerts from one agent (or Claktory), none urgent,
  /// folded into one row ([alertFoldAt]).
  alertGroup,
}

/// One thing that waits for the person's answer.
class NeedsYouItem {
  const NeedsYouItem.entry(this.agentId, this.seq)
    : kind = NeedsYouKind.entry,
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      sessionId = '',
      conversationId = '';

  const NeedsYouItem.chat(this.chatId, this.seq)
    : kind = NeedsYouKind.chat,
      agentId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      sessionId = '',
      conversationId = '';

  const NeedsYouItem.pipe(this.kind, this.pipeId, {this.seq = 0})
    : agentId = '',
      chatId = '',
      providerId = '',
      accountId = '',
      sessionId = '',
      conversationId = '';

  const NeedsYouItem.signIn(this.providerId, this.accountId)
    : kind = NeedsYouKind.signIn,
      agentId = '',
      chatId = '',
      pipeId = '',
      seq = 0,
      sessionId = '',
      conversationId = '';

  const NeedsYouItem.move()
    : kind = NeedsYouKind.move,
      agentId = '',
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0,
      sessionId = '',
      conversationId = '';

  const NeedsYouItem.vault()
    : kind = NeedsYouKind.vault,
      agentId = '',
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0,
      sessionId = '',
      conversationId = '';

  const NeedsYouItem.recoveryKey()
    : kind = NeedsYouKind.recoveryKey,
      agentId = '',
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0,
      sessionId = '',
      conversationId = '';

  const NeedsYouItem.browser(this.agentId, this.sessionId)
    : kind = NeedsYouKind.browser,
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0,
      conversationId = '';

  const NeedsYouItem.providerWait(this.providerId)
    : kind = NeedsYouKind.providerWait,
      agentId = '',
      chatId = '',
      pipeId = '',
      accountId = '',
      seq = 0,
      sessionId = '',
      conversationId = '';

  const NeedsYouItem.lidStuck()
    : kind = NeedsYouKind.lidStuck,
      agentId = '',
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0,
      sessionId = '',
      conversationId = '';

  const NeedsYouItem.keepAwake()
    : kind = NeedsYouKind.keepAwake,
      agentId = '',
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0,
      sessionId = '',
      conversationId = '';

  const NeedsYouItem.backendService()
    : kind = NeedsYouKind.backendService,
      agentId = '',
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0,
      sessionId = '',
      conversationId = '';

  const NeedsYouItem.dmSend(this.agentId, this.conversationId)
    : kind = NeedsYouKind.dmSend,
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0,
      sessionId = '';

  /// An alert ([NeedsYouKind.alert]), from [agentId] ('' for Claktory's
  /// own); its id is its [sessionId].
  const NeedsYouItem.alert(this.agentId, this.sessionId)
    : kind = NeedsYouKind.alert,
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0,
      conversationId = '';

  /// [agentId]'s alerts, folded ('' for Claktory's own).
  const NeedsYouItem.alertGroup(this.agentId)
    : kind = NeedsYouKind.alertGroup,
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0,
      sessionId = '',
      conversationId = '';

  /// The place's root is its [sessionId].
  const NeedsYouItem.protectedPlace(this.sessionId)
    : kind = NeedsYouKind.protectedPlace,
      agentId = '',
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0,
      conversationId = '';

  /// The waiting device's id is its [sessionId].
  const NeedsYouItem.device(this.sessionId)
    : kind = NeedsYouKind.device,
      agentId = '',
      chatId = '',
      pipeId = '',
      providerId = '',
      accountId = '',
      seq = 0,
      conversationId = '';

  final NeedsYouKind kind;
  final String agentId;
  final String chatId;
  final String pipeId;

  /// The provider and account to sign in to again.
  final String providerId;
  final String accountId;

  /// The browser session waiting, the device, or the alert.
  final String sessionId;

  /// The direct-message conversation an agent's messages wait to go to.
  final String conversationId;

  /// The entry's seq in the agent's or chat's conversation, the held
  /// message's in the pipe's, or the checklist item's number.
  final int seq;

  /// Whether it's a card in a conversation, an agent's or a chat's.
  bool get isEntry => kind == NeedsYouKind.entry || kind == NeedsYouKind.chat;

  /// What tells it apart from every other.
  String get key => switch (kind) {
    NeedsYouKind.entry => 'agent:$agentId/$seq',
    NeedsYouKind.chat => 'chat:$chatId/$seq',
    NeedsYouKind.pipeHeld => 'pipe:$pipeId/$seq',
    NeedsYouKind.pipeUnanswered => 'pipe:$pipeId/unanswered/$seq',
    NeedsYouKind.pipeApproval => 'pipe:$pipeId/approval',
    NeedsYouKind.pipeLoop => 'pipe:$pipeId/loop',
    NeedsYouKind.pipeIdentity => 'pipe:$pipeId/identity',
    NeedsYouKind.signIn => 'signin:$providerId/$accountId',
    NeedsYouKind.vault => 'vault',
    NeedsYouKind.move => 'move',
    NeedsYouKind.recoveryKey => 'recoveryKey',
    NeedsYouKind.browser => 'browser:$sessionId',
    NeedsYouKind.dmSend => 'dm:$agentId/$conversationId',
    NeedsYouKind.providerWait => 'usage:$providerId',
    NeedsYouKind.device => 'device:$sessionId',
    NeedsYouKind.keepAwake => 'keepAwake',
    NeedsYouKind.backendService => 'backendService',
    NeedsYouKind.lidStuck => 'lidStuck',
    NeedsYouKind.protectedPlace => 'protected:$sessionId',
    NeedsYouKind.alert => 'alert:$sessionId',
    NeedsYouKind.alertGroup => 'alerts:$agentId',
  };

  /// An alert, or a fold of them: a row of its own, read rather than
  /// answered.
  bool get isAlert =>
      kind == NeedsYouKind.alert || kind == NeedsYouKind.alertGroup;

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
      if (archivedProjectOf(workspace, agent) == null) ...[
        // A checklist item waiting on the person shows only as the card
        // asking about it, never on its own (Matt's ruling).
        for (final seq in agent.waitingSeqs)
          NeedsYouItem.entry(agent.id, seq.toInt()),
      ],
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
        if (pipeAccountChanged(pipe))
          NeedsYouItem.pipe(NeedsYouKind.pipeIdentity, pipe.id),
        for (final seq in pipe.waitingSeqs)
          NeedsYouItem.pipe(NeedsYouKind.pipeHeld, pipe.id, seq: seq.toInt()),
        for (final seq in pipe.unansweredSeqs)
          NeedsYouItem.pipe(
            NeedsYouKind.pipeUnanswered,
            pipe.id,
            seq: seq.toInt(),
          ),
      ],
    for (final provider in workspace.providers)
      for (final account in provider.accounts)
        if (account.signInAgain) NeedsYouItem.signIn(provider.id, account.id),
    // Everyone waiting on a provider's limit or credit, one item each.
    for (final wait in workspace.providerWaits)
      NeedsYouItem.providerWait(wait.providerId),
    if (workspace.vault.agentsWaiting) const NeedsYouItem.vault(),
    if (workspace.vault.recoveryKeyUnconfirmed)
      const NeedsYouItem.recoveryKey(),
    // Asked once, on a laptop agents have worked on.
    if (workspace.keepAwake.ask) const NeedsYouItem.keepAwake(),
    // Started by hand: Restore hands it back to its service.
    if (backendByHand(workspace)) const NeedsYouItem.backendService(),
    // A laptop that can't sleep with its lid closed: factoryd notifies too.
    if (workspace.keepAwake.lidClosed == LidClosedState.LID_CLOSED_STUCK)
      const NeedsYouItem.lidStuck(),
    // A place macOS refused agents, once per refusal; on every device, as
    // the Mac's own words say where to turn it on.
    for (final place in workspace.protectedPlaces)
      if (place.needsYou) NeedsYouItem.protectedPlace(place.root),
    // Also floating over the window on every device (MoveGate).
    if (workspace.hasMove() && moveStatusOf(workspace.move).awaitsPerson)
      const NeedsYouItem.move(),
    // Only while it waits for the person's choice on their Mac; running,
    // it shows in its agent's conversation.
    for (final session in workspace.browserSessions)
      if (session.state == 'pending' &&
          workspace.agents.any(
            (agent) =>
                agent.id == session.agentId &&
                archivedProjectOf(workspace, agent) == null,
          ))
        NeedsYouItem.browser(session.agentId, session.id),
    // An agent's messages to a friend, one item per agent and friend.
    for (final send in {
      for (final send in workspace.dmSends)
        if (workspace.agents.any(
          (agent) =>
              agent.id == send.agentId &&
              archivedProjectOf(workspace, agent) == null,
        ))
          (send.agentId, send.conversationId),
    })
      NeedsYouItem.dmSend(send.$1, send.$2),
    // Devices waiting to be allowed, one item each.
    for (final device in workspace.remote.pendingDevices)
      NeedsYouItem.device(device.deviceId),
    // Alerts, urgent ones first: urgent ones alone, the rest folded by who
    // sent them from three on (§3.9). Where each goes in a list is the
    // list's (urgent above the cards, the rest under them).
    ...alertItems(workspace),
  ];
}

/// The device [item] is about, while it waits.
PendingDevice? pendingDeviceOf(Workspace workspace, NeedsYouItem item) =>
    workspace.remote.pendingDevices
        .where((device) => device.deviceId == item.sessionId)
        .firstOrNull;

/// The messages [item] stands for, oldest first, while the backend lists
/// them: never their text, only whom they go to.
List<DmSendWaiting> dmSendsOf(Workspace workspace, NeedsYouItem item) => [
  for (final send in workspace.dmSends)
    if (send.agentId == item.agentId &&
        send.conversationId == item.conversationId)
      send,
];

/// What a waiting agent message asks (Trish's words): "Send June's message
/// to Ana?", "Send 3 messages from June to Ana?"; the friend by their tag
/// when they have no name.
String dmSendSummary(Workspace workspace, NeedsYouItem item) {
  final sends = dmSendsOf(workspace, item);
  if (sends.isEmpty) return 'No longer needs you';
  final agent = workspace.agents
      .where((agent) => agent.id == item.agentId)
      .firstOrNull;
  // With their tag, which they can't choose, as the chat's header: a
  // name alone could be anyone's (Fugo).
  final send = sends.first;
  final named = send.friendName.trim();
  final friend = named.isNotEmpty && send.friendTag.isNotEmpty
      ? '$named · ${send.friendTag}'
      : named.isNotEmpty
      ? named
      : send.friendTag.isNotEmpty
      ? send.friendTag
      : 'your friend';
  final name = agent?.name ?? '';
  if (sends.length > 1) {
    return 'Send ${sends.length} messages from '
        '${name.isEmpty ? 'your agent' : name} to $friend?';
  }
  return "Send ${name.isEmpty ? 'your agent' : "$name's"} message to "
      '$friend?';
}

/// The checklist items the card at [seq] in [agentId]'s conversation asks
/// about (WaitingItem.card_agent_id and card_seq), each with the agent
/// whose checklist holds it.
List<({String checklistAgentId, WaitingItem item})> cardItemsOf(
  Workspace workspace,
  String agentId,
  int seq,
) => [
  for (final agent in workspace.agents)
    for (final waiting in agent.waitingItems)
      if (waiting.cardSeq != 0 &&
          waiting.cardSeq.toInt() == seq &&
          (waiting.cardAgentId.isEmpty ? agent.id : waiting.cardAgentId) ==
              agentId)
        (checklistAgentId: agent.id, item: waiting),
];

/// The context above a card about a checklist item: "On its checklist:
/// Pick the icon", or "On Bruno's checklist: …" when the item is on
/// another agent's checklist than the conversation the card is in.
String cardItemWords(
  Workspace workspace, {
  required String checklistAgentId,
  required String cardAgentId,
  required WaitingItem item,
}) {
  // The agent's words, so what reorders them shows by its code (Fugo).
  final text = proseCodes(item.text);
  if (checklistAgentId == cardAgentId) return 'On its checklist: $text';
  final name =
      workspace.agents
          .where((agent) => agent.id == checklistAgentId)
          .firstOrNull
          ?.name ??
      checklistAgentId;
  return "On $name's checklist: $text";
}

/// The browser session [item] waits on, while the backend lists it.
BrowserSession? browserSessionOf(Workspace workspace, NeedsYouItem item) {
  for (final session in workspace.browserSessions) {
    if (session.id == item.sessionId) return session;
  }
  return null;
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
int needsYouCount(Workspace workspace) =>
    needsYouItems(workspace).where((item) => !item.isAlert).length +
    unreadAlerts(workspace);

/// What the count says to a screen reader.
String needsYouLabel(int count) => count == 1
    ? 'Something is waiting for your answer'
    : '$count things are waiting for your answer';

/// The Needs you page: on a Mac a pane beside the conversation (or the
/// agent view's place, with no agent), on a phone a page of its own.
class InboxView extends StatefulWidget {
  const InboxView({super.key, this.onOpen});

  /// What a tap on a request does instead of opening it in place: an
  /// iPhone's (and a narrow iPad's) opens it alone on its own screen
  /// (#599, needs-you-cards.md §4.5), as [NeedsYouList.onOpen].
  final void Function(NeedsYouItem item)? onOpen;

  @override
  State<InboxView> createState() => _InboxViewState();
}

class _InboxViewState extends State<InboxView> {
  /// Whether it shows History, what the person already decided, in place
  /// of what waits. Each opening starts at what waits, unless the person
  /// just opened a History row (Backend.needsYouHistoryNext).
  late var _history = () {
    final backend = AppScope.of(context);
    final next = backend.needsYouHistoryNext;
    backend.needsYouHistoryNext = false;
    return next;
  }();

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    void close() => AppScope.closePage(context);
    final inPane = InPane.of(context);
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: FocusScope(
        autofocus: !inPane,
        // A click on its header or tabs makes its list the one ⌘A selects.
        child: SelectionPane(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (!inPane)
                isPhoneLayout(context)
                    ? PhoneHeader(
                        leading: HeaderButton.back(onTap: close),
                        title: Text(
                          'Needs you',
                          style: PhoneType.emphasis.copyWith(
                            color: palette.text,
                          ),
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
                              // At the largest text in a narrow window it
                              // ends in "…" rather than pushing Done off.
                              Expanded(
                                child: Text(
                                  'Needs you',
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TypeScale.of(context).label.copyWith(
                                    color: palette.text,
                                    fontWeight: FontWeight.w600,
                                  ),
                                ),
                              ),
                              Button(label: 'Done', onPressed: close),
                            ],
                          ),
                        ),
                      ),
              // What waits, or what was already decided.
              Padding(
                padding: isPhoneLayout(context)
                    ? const EdgeInsets.fromLTRB(Space.l, Space.s, Space.l, 0)
                    : const EdgeInsets.fromLTRB(Space.xl, Space.m, Space.xl, 0),
                child: Align(
                  alignment: Alignment.topCenter,
                  child: ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 640),
                    child: Builder(
                      builder: (context) {
                        final tabs = NeedsYouTabs(
                          history: _history,
                          choose: (history) =>
                              setState(() => _history = history),
                        );
                        final focus = !_history && focusModeHere(context);
                        // A narrow iPad's tabs take the width: at the
                        // largest text, Focus mode goes under them, at the
                        // end.
                        if (focus &&
                            isPhoneLayout(context) &&
                            largeText(context)) {
                          return Column(
                            crossAxisAlignment: CrossAxisAlignment.stretch,
                            children: [
                              tabs,
                              const Align(
                                alignment: Alignment.centerRight,
                                child: FocusModeButton(),
                              ),
                            ],
                          );
                        }
                        return Row(
                          children: [
                            Expanded(
                              child: Align(
                                alignment: Alignment.centerLeft,
                                child: tabs,
                              ),
                            ),
                            if (focus) const FocusModeButton(),
                          ],
                        );
                      },
                    ),
                  ),
                ),
              ),
              Expanded(
                child: _history
                    // Its words select and copy, as what waits does; each
                    // row is copied apart from the next.
                    ? const SelectableArea(child: NeedsYouHistoryList())
                    : NeedsYouList(onOpen: widget.onOpen),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Whether Needs you has a way into focus mode here: a Mac, an iPad (a
/// narrow window too, where its sitting is the phone's one-card screen),
/// a Windows PC. Never on a phone (#606, Matt: a phone's list, a request
/// tapped opening alone, is already focus mode): an iPhone's idiom, not its
/// layout, so a narrow iPad keeps its button; Android never had it.
bool focusModeHere(BuildContext context) =>
    defaultTargetPlatform != TargetPlatform.android &&
    deviceIdiom(context) != Idiom.phone;

/// The page's way into focus mode (#514), as ⌘⇧N, at the end of its tabs:
/// shown while something waits, since focus mode shows only what waits.
class FocusModeButton extends StatelessWidget {
  const FocusModeButton({super.key});

  static const label = 'Focus on Needs you';

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        if (needsYouCount(backend.workspace.workspace) == 0) {
          return const SizedBox.shrink();
        }
        return Padding(
          padding: const EdgeInsets.only(left: Space.s),
          child: HoverCard(
            end: true,
            card: (context) => const Text('$label · ⌘⇧N'),
            child: Button(
              label: 'Focus mode',
              semanticLabel: label,
              // It grows with its label, as an SF Symbol does with
              // Dynamic Type, up to where the label stops (2× on a phone).
              glyph: (color) {
                final size = MediaQuery.textScalerOf(context)
                    .clamp(
                      maxScaleFactor: isPhoneLayout(context)
                          ? 2
                          : double.infinity,
                    )
                    .scale(16);
                return SizedBox.square(
                  dimension: size,
                  child: FittedBox(child: FocusGlyph(color: color)),
                );
              },
              onPressed: () => openFocusMode(context),
            ),
          ),
        );
      },
    );
  }
}

/// Everything that waits for the person, newest first, each a bubble that
/// opens to its card. What they answer here stays, answered, until they
/// leave, so they see it went through. A phone's drawer can show it too.
class NeedsYouList extends StatefulWidget {
  const NeedsYouList({
    super.key,
    this.padding,
    this.shrinkWrap = false,
    this.onOpen,
  });

  /// What a tap on a request does instead of opening it in place (an
  /// iPhone's: it alone, on its own screen). It stays in the list once
  /// answered, showing how it ended, as one answered in place does.
  final void Function(NeedsYouItem item)? onOpen;

  /// Around the list; the page's own by default.
  final EdgeInsets? padding;

  /// As tall as its items, up to what it's given (the chip's drop-down).
  final bool shrinkWrap;

  @override
  State<NeedsYouList> createState() => _NeedsYouListState();
}

class _NeedsYouListState extends State<NeedsYouList> {
  late final Backend _backend = AppScope.of(context);

  /// The cards as fetched, shared with focus mode (#514).
  late final NeedsYouEntries _store = NeedsYouEntries.of(_backend);

  /// Open bubbles, and what the person opened here: kept in the list once
  /// answered, until they leave, showing how it ended ([NeedsYouEntries.hold]).
  final _open = <String>{};
  final _kept = <String, NeedsYouItem>{};

  /// Alerts read here, by id: they stay, marked Read, until the person
  /// leaves (§3.9), as answered cards do.
  final _readHere = <String, Alert>{};

  /// The alert row just opened from its notification, standing out a
  /// moment ([NeedsYouEntries.revealAlert]).
  String? _highlighted;
  Timer? _unhighlight;

  /// Find in the list (⌘F): who asks and what, as the bubbles say them.
  late final _find = LocalFind(
    placeholder: 'Search Needs you',
    items: () => [
      for (final bubble in _shown)
        (
          bubble.item.key,
          () {
            final (:who, :what) = bubble.words(_backend);
            return '$who\n$what';
          }(),
        ),
      for (final (item, words) in _shownAlerts) (item.key, words),
    ],
    go: (key, _) {
      final context = _places[key]?.currentContext;
      if (context == null) return;
      Scrollable.ensureVisible(
        context,
        alignment: 0.2,
        alignmentPolicy: ScrollPositionAlignmentPolicy.keepVisibleAtEnd,
      );
      Scrollable.ensureVisible(
        context,
        alignmentPolicy: ScrollPositionAlignmentPolicy.keepVisibleAtStart,
      );
    },
  );

  /// The bubbles as the last build showed them, and each one's place.
  var _shown = const <_Bubble>[];

  /// The alert rows as the last build showed them, with their words.
  var _shownAlerts = const <(NeedsYouItem, String)>[];
  final _places = <String, GlobalKey>{};

  /// The list holds still under the finger (#599, row 688; Lucy's §4.5):
  /// a row coming in above the ones in view goes in with the offset grown
  /// by its height (needs_you_hold.dart), and one leaving fades where it
  /// was, the rows under it closing up only once no finger is down.
  final _scroll = ScrollController();
  final _content = GlobalKey();

  /// Every row this list has shown, and those that came in since it
  /// opened and haven't been in view yet: what the pill counts.
  Set<String>? _known;
  final _fresh = <String>{};
  var _kinds = const <String, ({bool alert, bool urgent})>{};
  var _above = (requests: 0, alerts: 0, urgent: 0);
  var _padTop = 0.0;

  /// Each request's time as last drawn, and those whose time rose since
  /// (changed by their agent, or replied on), which move up the list: the
  /// list holds the rows in view for them as for one that came in, and
  /// the pill counts them (Squalo's R1).
  var _times = const <String, int>{};
  var _movedUp = const <String>{};

  /// The rows in their order as last drawn, each as drawn, and those that
  /// left, drawn faded in their place meanwhile.
  var _lastOrder = const <String>[];
  var _lastRows = const <String, Widget>{};
  final _leaving = <String, Widget>{};
  var _touches = 0;
  Timer? _closeUp;

  @override
  void initState() {
    super.initState();
    _scroll.addListener(_seeAbove);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _store.addListener(_rebuild);
      _store.revealAlert.addListener(_reveal);
      setState(() {});
      _reveal();
    });
  }

  @override
  void dispose() {
    _closeUp?.cancel();
    _scroll.dispose();
    _unhighlight?.cancel();
    _find.dispose();
    _store.revealAlert.removeListener(_reveal);
    _store.removeListener(_rebuild);
    for (final key in _kept.keys) {
      _store.release(key);
    }
    super.dispose();
  }

  void _rebuild() {
    if (mounted) setState(() {});
  }

  /// Each row's box by its key, for the list to hold the top one still.
  Map<String, RenderBox> _rowBoxes() => {
    for (final MapEntry(:key, :value) in _places.entries)
      if (value.currentContext?.findRenderObject() case final RenderBox box)
        key: box,
  };

  /// What came in above the rows in view and isn't seen yet: the pill's.
  /// One that comes into view, or went in below it, is seen.
  void _seeAbove() {
    if (!mounted) return;
    var requests = 0, alerts = 0, urgent = 0;
    final content = _content.currentContext?.findRenderObject();
    if (_fresh.isNotEmpty && content is RenderBox && _scroll.hasClients) {
      final top = _scroll.offset - _padTop;
      for (final key in [..._fresh]) {
        final box = _places[key]?.currentContext?.findRenderObject();
        if (box is! RenderBox || !box.attached || !box.hasSize) {
          _fresh.remove(key);
          continue;
        }
        // Seen once its top is in view: at the very top, the offset grown
        // by its height still shows the padding's worth of its foot.
        final at = box.getTransformTo(content).getTranslation().y;
        if (at >= top - 1) {
          _fresh.remove(key);
          continue;
        }
        final kind = _kinds[key];
        if (kind == null) continue;
        if (kind.urgent) urgent++;
        kind.alert ? alerts++ : requests++;
      }
    } else {
      _fresh.clear();
    }
    final above = (requests: requests, alerts: alerts, urgent: urgent);
    if (above != _above) setState(() => _above = above);
  }

  /// Rows that showed and went above the top so the rows under one that
  /// came in could stay: counted by the pill until they're seen again.
  void _pushedAbove(List<String> keys) {
    if (!mounted) return;
    _fresh.addAll(keys);
    _seeAbove();
  }

  /// Up to what came in above.
  void _toNew() {
    if (!_scroll.hasClients) return;
    final to = _scroll.position.minScrollExtent;
    if (MediaQuery.disableAnimationsOf(context)) {
      _scroll.jumpTo(to);
    } else {
      unawaited(
        _scroll.animateTo(to, duration: Motion.base, curve: Curves.easeOut),
      );
    }
  }

  /// Rows that left close up once they've faded and no finger is down.
  void _closeUpSoon() {
    _closeUp?.cancel();
    _closeUp = Timer(Motion.base, _closeUpNow);
  }

  void _closeUpNow() {
    _closeUp = null;
    if (!mounted || _touches > 0 || _leaving.isEmpty) return;
    setState(_leaving.clear);
  }

  void _touched(int by) {
    _touches = math.max(0, _touches + by);
    if (_touches == 0 && _leaving.isNotEmpty && _closeUp == null) {
      _closeUpNow();
    }
  }

  void _toggle(NeedsYouItem item) {
    final key = item.key;
    setState(() {
      if (!_open.remove(key)) {
        _open.add(key);
        if (_kept[key] == null) _store.hold(item);
        _kept[key] = item;
      }
    });
    // Opened: as it is now.
    if (_open.contains(key)) _store.fetch(item);
  }

  /// [item] alone, elsewhere ([NeedsYouList.onOpen]): kept here to show
  /// how it ended once answered.
  void _openAlone(NeedsYouItem item) {
    if (_kept[item.key] == null) {
      _store.hold(item);
      setState(() => _kept[item.key] = item);
    }
    // As it is now, as one opened in place is: a copy fetched before its
    // agent changed it would be answered as it was, and turned away
    // (Squalo's B4).
    _store.fetch(item);
    widget.onOpen!(item);
  }

  /// Reads [alerts], [item]'s: they stay here, marked Read.
  void _readAlerts(NeedsYouItem item, List<Alert> alerts) {
    if (alerts.isEmpty) return;
    setState(() {
      for (final alert in alerts) {
        _readHere[alert.id] = alert;
      }
      _kept[item.key] = item;
    });
    fireAndLog(_backend.readAlerts([for (final alert in alerts) alert.id]));
  }

  /// An alert's notification was tapped: its row, in view, standing out a
  /// moment, and read (a fold opened, all of it: "June sent 3 more").
  void _reveal() {
    final id = _store.revealAlert.value;
    if (id == null || !mounted) return;
    _store.revealAlert.value = null;
    final workspace = _backend.workspace.workspace;
    final alert = workspace.alerts.where((alert) => alert.id == id).firstOrNull;
    if (alert == null) return;
    final item = alertItems(workspace).firstWhere(
      (item) => alertsOf([alert], item).isNotEmpty,
      orElse: () => NeedsYouItem.alert(alert.agentId, alert.id),
    );
    if (item.kind == NeedsYouKind.alertGroup) _open.add(item.key);
    _readAlerts(item, alertsOf(workspace.alerts, item));
    _unhighlight?.cancel();
    setState(() => _highlighted = item.key);
    _unhighlight = Timer(const Duration(seconds: 2), () {
      if (mounted) setState(() => _highlighted = null);
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final context = _places[item.key]?.currentContext;
      if (context != null && context.mounted) {
        Scrollable.ensureVisible(context, alignment: 0.2);
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final workspace = _backend.workspace.workspace;
    final waiting = needsYouItems(workspace);
    final keys = {for (final item in waiting) item.key};
    // A fold read here (all or some of it) stays whole: its agent's alerts
    // left unread show in it, not apart.
    final keptFolds = {
      for (final item in _kept.values)
        if (item.kind == NeedsYouKind.alertGroup && !keys.contains(item.key))
          item.agentId,
    };
    final alerts = [
      ...workspace.alerts,
      for (final alert in _readHere.values)
        if (!workspace.alerts.any((each) => each.id == alert.id)) alert,
    ];
    final unread = {for (final alert in workspace.alerts) alert.id};
    final all = [
      for (final item in waiting)
        if (!(item.kind == NeedsYouKind.alert &&
            keptFolds.contains(item.agentId) &&
            !isUrgentAlert(alerts, item)))
          item,
      // Answered here meanwhile: stays, showing how it ended.
      for (final item in _kept.values)
        if (!keys.contains(item.key) && _stillThere(item, workspace)) item,
    ];
    int latest(NeedsYouItem item) =>
        alertsOf(alerts, item).firstOrNull?.sentMs.toInt() ?? 0;
    // Urgent alerts above the cards, the rest under them (§3.9); each
    // newest first.
    final urgent = [
      for (final item in all)
        if (isUrgentAlert(alerts, item)) item,
    ]..sort((a, b) => latest(b).compareTo(latest(a)));
    final lateAlerts = [
      for (final item in all)
        if (item.isAlert && !isUrgentAlert(alerts, item)) item,
    ]..sort((a, b) => latest(b).compareTo(latest(a)));
    final items = [
      for (final item in all)
        if (!item.isAlert) item,
    ];
    // Newest first; unknown times (a pipe's approval, its pause) on top.
    // Each by the newest time this list has seen for it: a card's time
    // only rises, so a copy of it older than one shown (B3) never sinks
    // its row back down under the finger (Zucchero's row 717, Bruno 50306).
    final times = {
      for (final item in items)
        item.key: math.max(
          _times[item.key] ?? 0,
          _store.timeOf(item, workspace),
        ),
    };
    final order = {for (var i = 0; i < items.length; i++) items[i].key: i};
    // What stops agents first, under the urgent alerts, as focus mode's
    // queue has it; then the requests.
    items.sort((a, b) {
      final ua = focusUrgent(a), ub = focusUrgent(b);
      if (ua != ub) return ua ? -1 : 1;
      final ta = times[a.key]!;
      final tb = times[b.key]!;
      final ka = ta == 0 ? 1 << 62 : ta, kb = tb == 0 ? 1 << 62 : tb;
      if (ka != kb) return kb.compareTo(ka);
      return order[a.key]!.compareTo(order[b.key]!);
    });
    final movedUp = {
      for (final MapEntry(:key, value: time) in times.entries)
        if ((_times[key] ?? 0) > 0 && time > _times[key]!) key,
    };
    _times = times;
    _movedUp = movedUp;
    if (movedUp.isNotEmpty) {
      _fresh.addAll(movedUp);
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (identical(_movedUp, movedUp)) _movedUp = const {};
      });
    }
    final padding =
        widget.padding ??
        EdgeInsets.all(isPhoneLayout(context) ? Space.l : Space.xl);
    _shown = [
      for (final item in items)
        _Bubble(
          item: item,
          waiting: keys.contains(item.key),
          open: _open.contains(item.key),
          onToggle: () => item.isAction
              ? actOnNeedsYouItem(context, item)
              : widget.onOpen == null
              ? _toggle(item)
              : _openAlone(item),
          entry: item.isEntry ? _store.entryOf(item) : null,
          message:
              item.kind == NeedsYouKind.pipeHeld ||
                  item.kind == NeedsYouKind.pipeUnanswered
              ? _store.messageOf(item)
              : null,
          failed: _store.failedOf(item),
          workspace: workspace,
        ),
    ];
    Widget alertRow(NeedsYouItem item) => NeedsYouAlert(
      item: item,
      alerts: alertsOf(alerts, item),
      unread: unread,
      open: _open.contains(item.key),
      onToggle: () => setState(() {
        if (!_open.remove(item.key)) _open.add(item.key);
      }),
      onRead: (read) => _readAlerts(item, read),
      highlighted: _highlighted == item.key,
    );
    _shownAlerts = [
      for (final item in [...urgent, ...lateAlerts])
        (
          item,
          [
            for (final alert in alertsOf(alerts, item))
              '${alertFrom(alert)}\n${alertSummary(alert)}\n${alert.text}',
          ].join('\n'),
        ),
    ];
    // What's found follows what waits.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _find.refresh();
    });
    // An item answered here keeps its place, and with it its card's state
    // (how it ended), until it's no longer shown.
    final shownKeys = {
      for (final item in [...items, ...urgent, ...lateAlerts]) item.key,
    };
    // Came in since the list opened: new, until it's been in view.
    final known = _known;
    if (known == null) {
      _known = {...shownKeys};
    } else {
      for (final key in shownKeys) {
        if (known.add(key)) _fresh.add(key);
      }
    }
    _kinds = {
      for (final item in [...urgent, ...items, ...lateAlerts])
        item.key: (alert: item.isAlert, urgent: isUrgentAlert(alerts, item)),
    };
    // Gone (answered elsewhere, taken back): faded where it was meanwhile.
    for (final key in _lastOrder) {
      if (shownKeys.contains(key) || _leaving.containsKey(key)) continue;
      if (_lastRows[key] case final row?) _leaving[key] = row;
    }
    _leaving.removeWhere((key, _) => shownKeys.contains(key));
    if (_leaving.isNotEmpty && _closeUp == null) _closeUpSoon();
    _places.removeWhere(
      (key, _) => !shownKeys.contains(key) && !_leaving.containsKey(key),
    );
    Widget placed(NeedsYouItem item, Widget child) => Align(
      key: ValueKey(item.key),
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 640),
        child: Padding(
          padding: const EdgeInsets.only(bottom: Space.s),
          child: CopyShape(
            lines: true,
            child: LocalFindMarks(
              key: _places[item.key] ??= GlobalKey(),
              find: _find,
              itemKey: item.key,
              child: child,
            ),
          ),
        ),
      ),
    );
    if (items.isEmpty && urgent.isEmpty && lateAlerts.isEmpty) {
      return FindScope(
        target: _find,
        child: Center(
          child: Padding(
            padding: padding,
            child: Text(
              'Nothing is waiting for your answer.',
              textAlign: TextAlign.center,
              style: TextStyles.body.copyWith(color: palette.textMuted),
            ),
          ),
        ),
      );
    }
    // Every word in it selects and copies (drag, ⌘C, a right click's Copy;
    // a long press on a phone), as in a conversation; its buttons and
    // controls don't.
    // Copied, each item's lines on their own and a blank line between
    // items, as they read: the items in one Column inside the list, whose
    // own selection would run them together.
    // The rows in their order, each keyed by its item on the column's own
    // children: matched by place instead, a question coming in above
    // would take this one's card, and with it what the person was typing
    // and their focus.
    final rows = <(String, Widget)>[
      // Urgent alerts first: they can't wait (§3.9).
      for (final item in urgent) (item.key, placed(item, alertRow(item))),
      for (final bubble in _shown)
        (bubble.item.key, placed(bubble.item, bubble)),
      if (lateAlerts.isNotEmpty) ...[
        (
          'alerts-heading',
          Align(
            key: const ValueKey('alerts-heading'),
            alignment: Alignment.topCenter,
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 640, minWidth: 640),
              child: Padding(
                padding: EdgeInsets.fromLTRB(
                  Space.m,
                  _shown.isEmpty && urgent.isEmpty ? 0 : Space.l,
                  Space.m,
                  Space.xs,
                ),
                child: Semantics(
                  header: true,
                  child: Text(
                    'Alerts',
                    style: TextStyles.label.copyWith(
                      color: palette.textMuted,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
        for (final item in lateAlerts) (item.key, placed(item, alertRow(item))),
      ],
    ];
    _lastRows = {
      for (final (key, row) in rows)
        if (key != 'alerts-heading') key: row,
    };
    // Those that left, faded where they were, after the row before them.
    for (final (index, key) in _lastOrder.indexed) {
      final row = _leaving[key];
      if (row == null) continue;
      var at = 0;
      for (var before = index - 1; before >= 0; before--) {
        final found = rows.indexWhere((each) => each.$1 == _lastOrder[before]);
        if (found >= 0) {
          at = found + 1;
          break;
        }
      }
      rows.insert(at, (
        key,
        _Leaving(key: ValueKey('leaving $key'), child: row),
      ));
    }
    _lastOrder = [for (final (key, _) in rows) key];
    _padTop = padding.top;
    if (_fresh.isNotEmpty || _above.requests + _above.alerts > 0) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _seeAbove());
    }
    // Rows that open alone (an iPhone's, a narrow iPad's) are a list of
    // places to go, as Mail's are: a long press selects none of their words
    // (Zucchero's row 715). Opened in place, their cards' words select.
    Widget selectable(Widget child) => widget.onOpen != null
        ? SelectionContainer.disabled(child: child)
        : SelectableArea(child: child);
    return FindScope(
      target: _find,
      // A finger down holds the rows that left until it lifts.
      child: Listener(
        onPointerDown: (_) => _touched(1),
        onPointerUp: (_) => _touched(-1),
        onPointerCancel: (_) => _touched(-1),
        child: Stack(
          children: [
            selectable(
              CustomScrollView(
                controller: _scroll,
                shrinkWrap: widget.shrinkWrap,
                slivers: [
                  SliverPadding(
                    padding: padding,
                    sliver: HeldBox(
                      rows: _rowBoxes,
                      movedUp: () => _movedUp,
                      onPushedAbove: _pushedAbove,
                      // Selected where the pointer is at any zoom, however
                      // far it's scrolled.
                      child: ScrolledSelection(
                        child: CopyShape(
                          separator: '\n\n',
                          child: Column(
                            key: _content,
                            crossAxisAlignment: CrossAxisAlignment.stretch,
                            children: [for (final (_, row) in rows) row],
                          ),
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
            // What came in above the rows in view.
            Positioned(
              top: Space.s,
              left: 0,
              right: 0,
              child: Center(
                child: NewAbovePill(
                  requests: _above.requests,
                  alerts: _above.alerts,
                  urgent: _above.urgent,
                  onTap: _toNew,
                ),
              ),
            ),
          ],
        ),
      ),
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
        NeedsYouKind.browser => browserSessionOf(workspace, item) != null,
        NeedsYouKind.dmSend => workspace.agents.any(
          (agent) => agent.id == item.agentId,
        ),
        // Over once no one waits: its answer is that they carry on.
        NeedsYouKind.providerWait => false,
        // Answered here: how it ended stays on its card.
        NeedsYouKind.device => true,
        // Answered here: it stays, saying what the Mac does now.
        NeedsYouKind.keepAwake => workspace.keepAwake.available,
        // Restored: the backend that comes back runs as installed.
        NeedsYouKind.backendService => false,
        // Opened here, it stays once the Mac can sleep, saying so
        // (Abbacchio's row 1159).
        NeedsYouKind.lidStuck => true,
        // Set aside or let in here: it stays, saying which.
        NeedsYouKind.protectedPlace => true,
        // Read here: it stays, marked Read.
        NeedsYouKind.alert || NeedsYouKind.alertGroup => true,
        _ => workspace.pipes.any((pipe) => pipe.id == item.pipeId),
      };
}

/// One thing that waits, as a bubble: who asks and what, folded; opened,
/// the card itself, and a way to its conversation or pipe.
class _Bubble extends StatelessWidget with _ItemParts {
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

  @override
  final NeedsYouItem item;

  /// Still waiting; else answered here, showing how it ended.
  final bool waiting;
  final bool open;
  final VoidCallback onToggle;
  @override
  final Entry? entry;
  @override
  final PipeMessage? message;
  final String? failed;
  @override
  final Workspace workspace;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final agent = _agent;
    final liaisonOf = _liaisonOf;
    final (:who, :what) = words(backend);
    final named = needsYouWho(workspace, item, who);
    final large = largeText(context);
    // How it ended: its agent may have taken it back meanwhile (#407).
    final ask = entry?.whichKind() == Entry_Kind.askPerson
        ? entry!.askPerson
        : null;
    final proposal = entry?.whichKind() == Entry_Kind.proposal
        ? entry!.proposal
        : null;
    final secret = entry?.whichKind() == Entry_Kind.secretRequest
        ? entry!.secretRequest
        : null;
    final answered = Text(
      // How it ended, in its card's own words, never only "Answered"
      // (N12, Lucy's §8): Gave Hetzner, Couldn't do step 2, Replied…
      secret != null
          ? secret.state == SecretRequestState.SECRET_REQUEST_STATE_APPROVED
                ? 'Approved'
                : secretOutcome(secret, agent: agent?.name ?? secret.agentId)
          : ask != null
          ? askOutcome(ask, agent: agent?.name ?? 'The agent', short: true)
          : proposal?.withdrawn == true
          ? 'Withdrawn'
          : proposal?.state == ProposalState.PROPOSAL_STATE_DISMISSED
          ? 'Dismissed'
          : proposal != null
          ? (proposal.replied ? 'Replied' : 'Approved')
          : 'Answered',
      style: TextStyles.caption.copyWith(color: palette.textFaint),
    );
    final time = switch (item.kind) {
      NeedsYouKind.entry || NeedsYouKind.chat => waitingSince(entry),
      NeedsYouKind.pipeHeld ||
      NeedsYouKind.pipeUnanswered => message?.timestampMs.toInt() ?? 0,
      NeedsYouKind.browser =>
        browserSessionOf(workspace, item)?.createdMs.toInt() ?? 0,
      NeedsYouKind.dmSend =>
        dmSendsOf(workspace, item).firstOrNull?.createdMs.toInt() ?? 0,
      NeedsYouKind.device =>
        pendingDeviceOf(workspace, item)?.requestedMs.toInt() ?? 0,
      _ => 0,
    };
    final updated =
        (item.kind == NeedsYouKind.entry || item.kind == NeedsYouKind.chat) &&
        time > 0 &&
        updatedSince(entry) == time;
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
              // Who asks is said once, by the line beside it (Zucchero
              // 48776: "Lea, Lea, Project Manager").
              ExcludeSemantics(
                child: SizedBox.square(
                  dimension: 24,
                  child: Center(
                    child: agent != null
                        ? AgentCharacter(agent: agent, size: 24)
                        : item.kind == NeedsYouKind.providerWait
                        // The provider's own circle, its ring as full as it is.
                        ? switch (_providerWait) {
                            final wait? => switch (providerWaitMeters(
                              workspace,
                              wait,
                            ).firstOrNull) {
                              final meter? => UsageRing(meter: meter),
                              null => ExtensionGlyph(
                                icon: 'chart-column',
                                name: '',
                                color: palette.textMuted,
                              ),
                            },
                            null => ExtensionGlyph(
                              icon: 'chart-column',
                              name: '',
                              color: palette.textFaint,
                            ),
                          }
                        : item.kind == NeedsYouKind.keepAwake ||
                              item.kind == NeedsYouKind.backendService ||
                              item.kind == NeedsYouKind.lidStuck
                        ? ExtensionGlyph(
                            icon: 'laptop-minimal',
                            name: '',
                            color: waiting
                                ? palette.textMuted
                                : palette.textFaint,
                          )
                        : item.kind == NeedsYouKind.protectedPlace
                        ? ExtensionGlyph(
                            icon: 'folder-lock',
                            name: '',
                            color: waiting
                                ? palette.textMuted
                                : palette.textFaint,
                          )
                        : item.isAction || item.kind == NeedsYouKind.device
                        ? ExtensionGlyph(
                            icon:
                                item.kind == NeedsYouKind.vault ||
                                    item.kind == NeedsYouKind.recoveryKey
                                ? 'key-round'
                                : item.kind == NeedsYouKind.device
                                ? deviceGlyph(
                                    pendingDeviceOf(workspace, item)?.kind ??
                                        DeviceKind.DEVICE_KIND_PHONE,
                                  )
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
                      builder: (context, now) => NameRoleLine(
                        name: named.name,
                        // Quietly beside its name (§3.10), cut first.
                        role: named.role,
                        // Its name's matches marked when found (⌘F).
                        mark: (span) => FindHighlight.mark(context, span),
                        after: [
                          if (named.after.isNotEmpty)
                            TextSpan(text: named.after),
                          if (time > 0) ...[
                            // Changed by its agent last (#473): said so, in
                            // signal, timed from then.
                            if (updated)
                              TextSpan(
                                text: ' · $updatedMark',
                                style: TextStyles.caption.copyWith(
                                  color: palette.signal,
                                  fontWeight: FontWeight.w500,
                                ),
                              ),
                            TextSpan(
                              text:
                                  '${updated ? ' ' : ' · '}'
                                  '${formatAge(time, now)}',
                              style: TextStyles.caption.copyWith(
                                color: waiting && waitedADay(time, now)
                                    ? palette.signal
                                    : palette.textFaint,
                                fontWeight: FontWeight.w400,
                              ),
                            ),
                          ],
                        ],
                        // At large text it wraps rather than cutting the
                        // name ("Ri…" beside Answered).
                        wrap: large,
                        style: TextStyles.label.copyWith(
                          color: waiting ? palette.text : palette.textMuted,
                          fontWeight: FontWeight.w600,
                        ),
                        roleStyle: TextStyles.label.copyWith(
                          color: palette.textMuted,
                          fontWeight: FontWeight.w400,
                        ),
                      ),
                    ),
                    Text.rich(
                      FindHighlight.markText(
                        context,
                        what,
                        TextStyles.body.copyWith(
                          color: waiting
                              ? palette.textMuted
                              : palette.textFaint,
                        ),
                      ),
                      // At large text, all of it: a procedure's step
                      // isn't cut to "· S…".
                      maxLines: large ? null : (open ? 3 : 2),
                      overflow: large ? null : TextOverflow.ellipsis,
                    ),
                    // At large text, under what it asked: beside it
                    // there's no room.
                    if (large && !waiting) answered,
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
                        // As shown (#428).
                        revision: entry?.proposal.revision,
                      ),
                    ),
                  ),
                )
              else if (waiting || large)
                const SizedBox.shrink()
              else
                Padding(
                  padding: const EdgeInsets.only(right: Space.s),
                  child: answered,
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
                    NeedsYouCard(
                      item: item,
                      entry: entry,
                      message: message,
                      failed: failed,
                      workspace: workspace,
                    ),
                    // A move's card is all there is to it, and a device's
                    // and keeping the Mac awake's, and a message to a
                    // friend's has Open chat on it.
                    if (item.kind != NeedsYouKind.move &&
                        item.kind != NeedsYouKind.device &&
                        item.kind != NeedsYouKind.keepAwake &&
                        item.kind != NeedsYouKind.backendService &&
                        item.kind != NeedsYouKind.lidStuck &&
                        item.kind != NeedsYouKind.protectedPlace &&
                        item.kind != NeedsYouKind.dmSend) ...[
                      const SizedBox(height: Space.s),
                      Wrap(
                        spacing: Space.l,
                        children: [
                          _OpenLink(
                            label: switch (item.kind) {
                              NeedsYouKind.entry when liaisonOf != null =>
                                'Open pipe',
                              NeedsYouKind.entry ||
                              NeedsYouKind.browser => 'Open conversation',
                              NeedsYouKind.chat => 'Open chat',
                              NeedsYouKind.providerWait
                                  when _providerWait?.agentIds.isEmpty ??
                                      false =>
                                'Open chat',
                              NeedsYouKind.providerWait => 'Open conversation',
                              _ => 'Open pipe',
                            },
                            onTap: () => openNeedsYouItem(context, item),
                          ),
                          // The checklist item the card asks about, glowing
                          // in its checklist beside the conversation.
                          if (_cardItems.firstOrNull case final about?)
                            _OpenLink(
                              label: 'Show in checklist',
                              onTap: () {
                                final backend = AppScope.of(context);
                                AppScope.closePage(context);
                                fireAndLog(
                                  showChecklistItem(
                                    backend,
                                    about.checklistAgentId,
                                    about.item.item,
                                  ),
                                );
                              },
                            ),
                        ],
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
}

/// What a Needs you item is about, from the Workspace and its card: whose
/// it is, its chat or pipe, and the words that say it.
mixin _ItemParts {
  NeedsYouItem get item;
  Workspace get workspace;
  Entry? get entry;
  PipeMessage? get message;

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

  ProviderWait? get _providerWait => providerWaitOf(workspace, item.providerId);

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

  /// The checklist items an agent's card asks about ([cardItemsOf]).
  List<({String checklistAgentId, WaitingItem item})> get _cardItems =>
      item.kind == NeedsYouKind.entry
      ? cardItemsOf(workspace, item.agentId, item.seq)
      : const [];

  /// The role beside the name of the agent that asks (§3.10, Matt via
  /// Bruno 46581): empty for no role, and where no agent's name is who
  /// asks: Claktory's own items, chats, pipes and their peers, and a
  /// liaison, which asks as its pipe's.
  String get _role {
    final agent = _agent;
    if (agent == null || _liaisonOf != null) return '';
    return switch (item.kind) {
      NeedsYouKind.entry ||
      NeedsYouKind.move ||
      NeedsYouKind.alert ||
      NeedsYouKind.alertGroup => agent.role.trim(),
      _ => '',
    };
  }

  /// A pipe's liaison asks as the pipe's ("Kai · Acme launch").
  Pipe? get _liaisonOf {
    final agent = _agent;
    return agent == null || agent.pipeId.isEmpty
        ? null
        : [
            for (final pipe in workspace.pipes)
              if (pipe.id == agent.pipeId) pipe,
          ].firstOrNull;
  }

  /// Who asks, and what, as its header says them (and find searches).
  ({String who, String what}) words(Backend backend) {
    final agent = _agent;
    final chat = _chat;
    final pipe = _pipe;
    final liaisonOf = _liaisonOf;
    final signIn = signInOf(workspace, item);
    final who = item.kind == NeedsYouKind.providerWait
        ? providerWaitOf(workspace, item.providerId)?.title ?? ''
        : item.kind == NeedsYouKind.signIn
        ? 'Sign in to ${signIn?.provider.name ?? 'your provider'} again'
        : item.kind == NeedsYouKind.vault
        // Named, since each Claktory has its own key, even on one account
        // (Matt's two Claktories, Bruno 45499).
        ? 'Unlock the secrets on ${claktoryNameInText(backend)} with its '
              'recovery key'
        : item.kind == NeedsYouKind.recoveryKey
        ? 'Save the recovery key for ${claktoryNameInText(backend)}'
        : item.kind == NeedsYouKind.device
        ? 'Allow a new device?'
        : item.kind == NeedsYouKind.keepAwake
        ? keepAwakeTitle
        : item.kind == NeedsYouKind.backendService
        ? backendServiceTitle
        : item.kind == NeedsYouKind.lidStuck
        ? lidClosedStuck(backend)
        : item.kind == NeedsYouKind.protectedPlace
        ? protectedPlaceTitle(protectedPlaceAt(workspace, item.sessionId))
        : item.isAlert
        ? switch (alertsOf(workspace.alerts, item)) {
            [final latest, ...] when item.kind == NeedsYouKind.alertGroup =>
              '${alertFrom(latest)} · '
                  '${alertsOf(workspace.alerts, item).length} alerts',
            [final latest, ...] => alertFrom(latest),
            _ => agent?.name ?? 'Claktory',
          }
        : liaisonOf != null
        ? '${agent!.name} · ${liaisonOf.name}'
        : agent?.name ??
              (chat == null ? null : chatTitle(chat)) ??
              pipe?.name ??
              '';
    return (who: who, what: _summary(backend));
  }

  /// What it asks, in a line.
  String _summary(Backend backend) {
    final pipe = _pipe;
    switch (item.kind) {
      case NeedsYouKind.alert || NeedsYouKind.alertGroup:
        final alerts = alertsOf(workspace.alerts, item);
        if (alerts.isEmpty) return 'Read';
        return alertSummary(alerts.first);
      case NeedsYouKind.pipeApproval:
        return pipe == null
            ? 'Waiting for your approval'
            : '${pipePeerName(pipe, start: true)} wants to connect';
      case NeedsYouKind.pipeLoop:
        return 'Paused: agents kept talking on their own';
      case NeedsYouKind.pipeIdentity:
        return pipe == null
            ? 'Another account now'
            : '${pipePeerClaktory(pipe)} is on another account '
                  'now';
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
      case NeedsYouKind.pipeUnanswered:
        final message = this.message;
        if (message == null) return pipeUnanswered;
        return '$pipeUnanswered\n${message.text}';
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
        // Never shown or saved on Windows (approve_elsewhere.dart): the
        // row says where (Hermes 53678).
        return approvesElsewhere
            ? 'Your way back into your secrets if you ever lose access. '
                  'To save it, use your phone, tablet or Mac.'
            : 'Your way back into your secrets if you ever lose access. '
                  'Save it in a password manager, then confirm you have it.';
      case NeedsYouKind.dmSend:
        return dmSendSummary(workspace, item);
      case NeedsYouKind.providerWait:
        return providerWaitOf(workspace, item.providerId)?.text ??
            'They carry on.';
      case NeedsYouKind.device:
        final device = pendingDeviceOf(workspace, item);
        return device == null
            ? 'No longer waiting'
            : '${deviceCallingItself(device)} wants to connect';
      case NeedsYouKind.keepAwake:
        final keep = workspace.keepAwake;
        return keep.ask
            ? keepAwakeSummary(backend)
            : keepAwakeAnswered(backend, keep);
      case NeedsYouKind.backendService:
        return backendByHand(workspace)
            ? backendServiceSummary(workspace.backendService)
            : 'Claktory runs in the background again.';
      case NeedsYouKind.lidStuck:
        final keep = workspace.keepAwake;
        final fix = keep.lidClosedFix.replaceAll('`', '');
        return keep.lidClosed != LidClosedState.LID_CLOSED_STUCK || fix.isEmpty
            ? 'It can sleep again.'
            : fix;
      case NeedsYouKind.protectedPlace:
        return protectedPlaceSummary(
          protectedPlaceAt(workspace, item.sessionId),
        );
      case NeedsYouKind.browser:
        final session = browserSessionOf(workspace, item);
        if (session == null) return 'No longer needs you';
        final host = browserHost(session.origin);
        return browserAssists(session)
            ? 'Allow reading and using $host in Chrome?'
            : 'Allow reading $host in Chrome?';
      case NeedsYouKind.entry || NeedsYouKind.chat:
        final entry = this.entry;
        if (entry == null) return 'Waiting for your answer';
        return waitingSummary(entry, backend, agentId: item.agentId);
    }
  }
}

/// An item's parts without a widget ([needsYouWords]).
class _Words with _ItemParts {
  _Words(this.item, this.workspace, {this.entry, this.message});

  @override
  final NeedsYouItem item;
  @override
  final Workspace workspace;
  @override
  final Entry? entry;
  @override
  final PipeMessage? message;
}

/// The role of the agent that asks for [item], to show quietly beside its
/// name (§3.10); empty where there's none to show.
String needsYouRole(Workspace workspace, NeedsYouItem item) =>
    _Words(item, workspace)._role;

/// [who] ([needsYouWords]) cut where its agent's role goes beside its
/// name (§3.10): "June" + "Project Manager" + " · 3 alerts". With no role
/// to show, [who] whole and nothing beside it.
({String name, String role, String after}) needsYouWho(
  Workspace workspace,
  NeedsYouItem item,
  String who,
) {
  final role = needsYouRole(workspace, item);
  final name = workspace.agents
      .where((agent) => agent.id == item.agentId)
      .firstOrNull
      ?.name;
  if (role.isEmpty || name == null || !who.startsWith(name)) {
    return (name: who, role: '', after: '');
  }
  return (name: name, role: role, after: who.substring(name.length));
}

/// Who asks, and what, as Needs you's bubble says them (and find searches):
/// for focus mode's queue (#514). [entry] and [message], its card as
/// fetched ([NeedsYouEntries]), when there is one.
({String who, String what}) needsYouWords(
  Backend backend,
  Workspace workspace,
  NeedsYouItem item, {
  Entry? entry,
  PipeMessage? message,
}) => _Words(item, workspace, entry: entry, message: message).words(backend);

/// [item]'s card with its real controls, as Needs you's bubble shows it
/// opened, for every kind; until its card is here, where it is ([failed]
/// when it couldn't be fetched). Focus mode's stage shows the same (#514).
class NeedsYouCard extends StatelessWidget with _ItemParts {
  const NeedsYouCard({
    super.key,
    required this.item,
    required this.entry,
    required this.message,
    required this.failed,
    required this.workspace,
  });

  @override
  final NeedsYouItem item;
  @override
  final Entry? entry;
  @override
  final PipeMessage? message;
  final String? failed;
  @override
  final Workspace workspace;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final pipe = _pipe;
    Widget line(String text, {Color? color}) => Text(
      text,
      style: TextStyles.caption.copyWith(color: color ?? palette.textFaint),
    );
    switch (item.kind) {
      case NeedsYouKind.alert || NeedsYouKind.alertGroup:
        final alerts = alertsOf(workspace.alerts, item);
        if (alerts.isEmpty) return line('Read');
        final backend = AppScope.of(context);
        return NeedsYouAlert(
          item: item,
          alerts: alerts,
          unread: {for (final alert in alerts) alert.id},
          open: true,
          onToggle: () {},
          onRead: (read) => fireAndLog(
            backend.readAlerts([for (final alert in read) alert.id]),
          ),
        );
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
      case NeedsYouKind.pipeIdentity:
        if (pipe == null || !pipeAccountChanged(pipe)) {
          return line(
            pipe == null ? 'This pipe was deleted.' : pipeStatus(pipe),
          );
        }
        // The pipe header's notice, with Trust this Claktory…
        return PipeIdentityNotice(pipe: pipe, inline: true);
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
      case NeedsYouKind.pipeUnanswered:
        final message = this.message;
        if (pipe == null) return line('This pipe was deleted.');
        if (message == null) {
          return line(
            failed ?? 'Loading…',
            color: failed == null ? null : palette.danger,
          );
        }
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            PipeMessageView(pipe: pipe, message: message),
            // Answered by no agent until one leads the pipe.
            if (pipe.leadId.isEmpty &&
                pipe.state != PipeState.PIPE_STATE_CLOSED) ...[
              const SizedBox(height: Space.s),
              line(pipeChooseLead),
              const SizedBox(height: Space.xs),
              Align(
                alignment: Alignment.centerLeft,
                child: Button(
                  label: 'Pipe settings…',
                  onPressed: () => showPipeSettings(context, pipe),
                ),
              ),
            ],
          ],
        );
      case NeedsYouKind.signIn ||
          NeedsYouKind.vault ||
          NeedsYouKind.recoveryKey:
        // Done in a click; never opened.
        return const SizedBox.shrink();
      case NeedsYouKind.providerWait:
        final wait = providerWaitOf(workspace, item.providerId);
        if (wait == null) return line('They carry on.');
        return ProviderWaitCard(wait: wait);
      case NeedsYouKind.keepAwake:
        final keep = workspace.keepAwake;
        if (!keep.ask) {
          return line(keepAwakeAnswered(AppScope.of(context), keep));
        }
        return const KeepAwakeCard();
      case NeedsYouKind.backendService:
        if (!backendByHand(workspace)) {
          return line('Claktory runs in the background again.');
        }
        return const BackendServiceRestore();
      case NeedsYouKind.lidStuck:
        final keep = workspace.keepAwake;
        if (keep.lidClosed != LidClosedState.LID_CLOSED_STUCK) {
          return line('It can sleep again.');
        }
        return LidClosedStuck(keep: keep, heading: false);
      case NeedsYouKind.protectedPlace:
        final place = protectedPlaceAt(workspace, item.sessionId);
        if (place == null) return line('No longer needs you.');
        if (!place.needsYou) return line(protectedPlaceEnded(place));
        return ProtectedPlaceCard(place: place);
      case NeedsYouKind.move:
        final move = _move;
        if (!MoveConfirmCard.shows(move)) {
          if (!move.active) return line('This move ended.');
          // Follow it: General, at the move (Lucy's Settings spec §5).
          return Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              line('This move is under way.'),
              _OpenLink(
                label: 'Follow it',
                onTap: () => AppScope.openSettingsAt(
                  context,
                  'general',
                  setting: 'move',
                ),
              ),
            ],
          );
        }
        return MoveConfirmCard(
          move: move,
          actions: BackendMove(AppScope.of(context)),
          agentName: _agent?.name ?? '',
        );
      case NeedsYouKind.dmSend:
        final sends = dmSendsOf(workspace, item);
        if (sends.isEmpty) return line('No longer needs you.');
        // Never its text here: it's read and sent only in the chat, where
        // Send and Don't send sit above the box. Open chat goes there in
        // one tap: opening it is the action.
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            line(
              sends.length > 1
                  ? 'Read them in the chat, then send them there.'
                  : 'Read it in the chat, then send it there.',
              color: palette.textMuted,
            ),
            const SizedBox(height: Space.s),
            Button(
              label: 'Open chat',
              primary: true,
              onPressed: () => openNeedsYouItem(context, item),
            ),
          ],
        );
      case NeedsYouKind.device:
        // Keyed by the device, so how it ended stays once it's gone.
        return AllowDeviceCard(
          key: ValueKey(item.key),
          device: pendingDeviceOf(workspace, item),
        );
      case NeedsYouKind.browser:
        final session = browserSessionOf(workspace, item);
        if (session == null) return line('No longer needs you.');
        return BrowserSessionCard(
          session: session,
          local: BrowserScope.localOf(context),
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
        final card =
            waitingCard(entry, agentId: item.agentId, chatId: item.chatId) ??
            line(
              item.kind == NeedsYouKind.chat
                  ? 'Open the chat to answer this.'
                  : 'Open the conversation to answer this.',
            );
        final about = _cardItems;
        if (about.isEmpty) return card;
        // What on the checklist the card is about, above it.
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (final (:checklistAgentId, item: waiting) in about)
              Padding(
                padding: const EdgeInsets.only(bottom: Space.s),
                child: Text.rich(
                  FindHighlight.mark(
                    context,
                    TextSpan(
                      text: cardItemWords(
                        workspace,
                        checklistAgentId: checklistAgentId,
                        cardAgentId: item.agentId,
                        item: waiting,
                      ),
                    ),
                  ),
                  style: TextStyles.caption.copyWith(color: palette.textMuted),
                ),
              ),
            card,
          ],
        );
    }
  }
}

/// What an action item's button says ([NeedsYouItem.isAction]).
String needsYouActionLabel(NeedsYouItem item) => switch (item.kind) {
  NeedsYouKind.signIn => 'Sign in again',
  NeedsYouKind.vault => 'Unlock secrets…',
  _ => 'Save recovery key…',
};

/// Does what an action item asks, with no card to open first: signs in
/// again (Settings → Providers shows how it goes), or opens the form that
/// unlocks the secrets or saves the recovery key.
void actOnNeedsYouItem(BuildContext context, NeedsYouItem item) {
  switch (item.kind) {
    case NeedsYouKind.signIn:
      final backend = AppScope.of(context);
      final account = signInAccount(backend.workspace.workspace, item);
      if (account == null) return;
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
      showModal(context, (context, close) => SaveRecoveryKeyForm(close: close));
    default:
  }
}

/// Opens where [item] is answered: the conversation at its card, the chat,
/// or the pipe; leaving the page it's opened from unless [closePage] is
/// false (focus mode leaves itself first, #514).
void openNeedsYouItem(
  BuildContext context,
  NeedsYouItem item, {
  bool closePage = true,
}) {
  if (item.kind == NeedsYouKind.dmSend) {
    // The chat over Needs you, from the person's own conversations; its
    // review card sends it.
    fireAndLog(openDmChat(context, item.conversationId));
    return;
  }
  final backend = AppScope.of(context);
  final parts = _Words(item, backend.workspace.workspace);
  final chat = parts._chat;
  if (closePage) AppScope.closePage(context);
  final agent = parts._agent;
  final pipe = parts._pipe;
  if (item.kind == NeedsYouKind.providerWait) {
    // The first that waits: its wait line has the same ways forward.
    final wait = providerWaitOf(backend.workspace.workspace, item.providerId);
    final waiting = wait == null ? null : firstWaiter(wait);
    if (wait != null && wait.agentIds.isNotEmpty) {
      fireAndLog(backend.selectAgent(waiting!));
    } else if (waiting != null && waiting.isNotEmpty) {
      final chat = [
        for (final chat in backend.workspace.workspace.chats)
          if (chat.id == waiting) chat,
      ].firstOrNull;
      if (chat != null) openChat(context, chat);
    }
  } else if (item.isAlert && item.pipeId.isEmpty) {
    // Its agent's conversation; Claktory's own alerts have none.
    if (item.agentId.isNotEmpty) fireAndLog(backend.selectAgent(item.agentId));
  } else if (item.kind == NeedsYouKind.browser) {
    // Its card shows in the agent's conversation the whole time.
    fireAndLog(backend.selectAgent(item.agentId));
  } else if (item.kind == NeedsYouKind.entry) {
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
      if (item.kind == NeedsYouKind.pipeHeld ||
          item.kind == NeedsYouKind.pipeUnanswered) {
        backend.reveal.reachExtra(
          liaison?.id ?? pipeChatRevealId(item.pipeId),
          pipeItemKey(item.seq),
        );
      }
    }());
  }
  // On a phone, the conversation is pushed over the list.
  PhoneNavigation.of(context)?.value = true;
}

/// A quiet link under an open bubble's card.
class _OpenLink extends StatelessWidget {
  const _OpenLink({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    // Underlined at rest, so it reads as something to click before the
    // pointer finds it, and where there's no pointer at all; a finger's
    // height on a touch screen.
    return Semantics(
      button: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) {
          final color = hovered ? palette.text : palette.textMuted;
          return ConstrainedBox(
            constraints: BoxConstraints(
              minHeight: touchFirst(context) ? Metrics.of(context).target : 0,
            ),
            child: Align(
              alignment: AlignmentDirectional.centerStart,
              widthFactor: 1,
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: Space.xs),
                child: Text(
                  label,
                  style: TypeScale.of(context).caption.copyWith(
                    color: color,
                    fontWeight: FontWeight.w500,
                    decoration: TextDecoration.underline,
                    decorationColor: color,
                  ),
                ),
              ),
            ),
          );
        },
      ),
    );
  }
}

/// What a waiting card in [entry] asks, in a line; [agentId] is whose
/// conversation shows it. It names a decision in an agent's or a device's
/// words, so what reorders them shows by its code (proseCodes, Fugo).
String waitingSummary(Entry entry, Backend backend, {String? agentId}) =>
    proseCodes(_waitingLine(entry, backend, agentId: agentId));

String _waitingLine(Entry entry, Backend backend, {String? agentId}) =>
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
      Entry_Kind.askPerson => askSummary(entry.askPerson),
      Entry_Kind.pipeStart => pipeStartSummary(entry.pipeStart),
      Entry_Kind.appGrant => appGrantSummary(
        entry.appGrant,
        backend.workspace.workspace,
      ),
      Entry_Kind.appAccess => appAccessSummary(
        entry.appAccess,
        backend.workspace.workspace,
      ),
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
  Entry_Kind.appGrant => AppGrantCard(
    key: ValueKey(entry.appGrant.id),
    entry.appGrant,
    agentId: agentId,
    seq: entry.seq.toInt(),
  ),
  Entry_Kind.appAccess => AppAccessCard(
    key: ValueKey(entry.appAccess.id),
    entry.appAccess,
    agentId: agentId,
    seq: entry.seq.toInt(),
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
/// you"). A click opens focus mode at the conversation's own request on a
/// computer and a wide iPad; elsewhere it drops the Needs you bubbles down
/// under it, over the conversation, which stays as it is; another click
/// folds them away. What
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
    // On a computer and a wide iPad, focus mode, at this conversation's
    // own request: one Needs you.
    if (needsYouIsFocusMode(context)) {
      final agent = AppScope.of(context).workspace.selectedAgent;
      final seqs = <int>[
        if (agent != null)
          for (final seq in agent.waitingSeqs) seq.toInt(),
      ];
      openFocusMode(
        context,
        at: agent == null || seqs.isEmpty
            ? null
            : NeedsYouItem.entry(agent.id, seqs.reduce(math.max)).key,
      );
      return;
    }
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
      // Its own element, which VoiceOver presses as a click does
      // (Abbacchio's row 1615).
      container: true,
      onTap: _toggle,
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
              // It drops down: a sheet from below, or focus mode, has no
              // need of it.
              if (widget.onOpen == null && !needsYouIsFocusMode(context)) ...[
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

/// A row that left the list while in view (answered elsewhere, taken
/// back): it fades where it was, taking no taps, until the list closes up
/// over it once no finger is down (#599, row 688).
class _Leaving extends StatelessWidget {
  const _Leaving({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) => IgnorePointer(
    child: ExcludeSemantics(
      child: TweenAnimationBuilder<double>(
        tween: Tween(begin: 1, end: 0),
        duration: MediaQuery.disableAnimationsOf(context)
            ? Duration.zero
            : Motion.base,
        curve: Curves.easeOut,
        builder: (context, shown, child) =>
            Opacity(opacity: shown, child: child),
        child: child,
      ),
    ),
  );
}
