// Needs you on an iPhone (#514, Formaggio's spec §7): the screen is focus
// mode itself, one card a screen. Its title says where the card is among
// those waiting ("3 of 7") and opens the queue as a sheet; ˄ ˅ move
// between cards, as Mail's do between messages; who asks is a row that
// pushes the card's conversation; and the answer is pinned at the foot,
// over the keyboard, its yes under the thumb. After an answer Claktory
// accepted, the next card comes by itself. The sitting's rules are the
// shared FocusSession (needs_you_focus.dart): only the layout is a
// phone's.
import 'dart:async';

import 'package:flutter/widgets.dart';

import '../card_answer.dart' show AnswerDock, CardAnswer;
import '../card_files.dart' show CardAttachButton;
import '../characters/character.dart';
import '../controls.dart';
import '../dictation.dart' show Dictation, DictationFor;
import '../identity.dart';
import '../backend.dart' show fireAndLog;
import '../inbox.dart'
    show
        NeedsYouCard,
        NeedsYouItem,
        NeedsYouKind,
        needsYouWho,
        needsYouWords,
        signInAccount,
        waitedADay;
import '../needs_you_focus.dart';
import '../needs_you_history.dart' show NeedsYouHistoryList, NeedsYouTabs;
import '../protocol/factory/v1/factory.pb.dart';
import '../recovery_key.dart' show SaveRecoveryKeyForm;
import '../scope.dart';
import '../secrets.dart' show UnlockSecretsForm;
import '../usage.dart' show formatAge;
import 'drawer.dart' show largeText;
import '../name_role_line.dart';
import 'haptics.dart';
import 'header.dart';
import 'metrics.dart';
import 'navigation.dart';
import 'presentation.dart' show SheetAction, showActions, showPanel;

/// The thumb row's height: the yes and the no, a finger's height and more.
const phoneThumbRow = 50.0;

/// The answer pinned at the foot (tests find its buttons by it).
const phoneThumbDockKey = Key('needs-you-thumb-dock');

/// Needs you on a phone: the sitting's cards, one a screen.
class PhoneNeedsYou extends StatefulWidget {
  const PhoneNeedsYou({
    super.key,
    this.session,
    this.onClose,
    this.reviewBody,
    this.onContext,
  });

  /// The sitting to show (an iPad's focus mode, kept above a resize, or a
  /// test's); else one of its own, for as long as the screen is up.
  final FocusSession? session;

  /// What Back does: leaves focus mode on an iPad; else the page closes.
  final VoidCallback? onClose;

  /// What a tap on who asks opens: the iPad's own Context, beside; else
  /// the card's conversation pushed over the card (§7.6).
  final VoidCallback? onContext;

  /// A review the platform shows itself (the iPad's documents, with
  /// Pencil), as [FocusStage.reviewBody]; null for the card's own Review.
  final FocusReviewBody? Function(
    BuildContext context,
    NeedsYouItem item,
    Entry card,
  )?
  reviewBody;

  @override
  State<PhoneNeedsYou> createState() => _PhoneNeedsYouState();
}

class _PhoneNeedsYouState extends State<PhoneNeedsYou> {
  FocusSession? _own;
  String? _receipt;

  FocusSession get _session => widget.session ?? _own!;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (widget.session != null) return;
    final backend = AppScope.of(context);
    if (_own?.backend == backend) return;
    _own?.removeListener(_changed);
    _own?.dispose();
    _own = FocusSession(backend)..addListener(_changed);
  }

  @override
  void initState() {
    super.initState();
    widget.session?.addListener(_changed);
  }

  @override
  void dispose() {
    widget.session?.removeListener(_changed);
    _own?.removeListener(_changed);
    _own?.dispose();
    super.dispose();
  }

  /// An answer accepted: felt, as a message sent is.
  void _changed() {
    final receipt = _session.receipt;
    if (receipt != null && receipt != _receipt) unawaited(Haptics.success());
    _receipt = receipt;
  }

  void _close() {
    final close = widget.onClose;
    close == null ? AppScope.closePage(context) : close();
  }

  @override
  Widget build(BuildContext context) {
    final session = _session;
    return ListenableBuilder(
      listenable: Listenable.merge([session, session.entries]),
      builder: (context, _) {
        final palette = Palette.of(context);
        final item = session.current;
        final (at, of) = session.position;
        // A file to review the platform shows itself (an iPad's
        // documents): answering the card itself, it takes all under who
        // asks, and its own bar answers, whatever its answer says (as
        // FocusStage, Sorbet 38658).
        final card = item != null && item.isEntry
            ? session.entries.entryOf(item)
            : null;
        final review =
            card != null &&
                card.hasAskPerson() &&
                card.askPerson.reviewPath.isNotEmpty
            ? card
            : null;
        final own = review == null
            ? null
            : widget.reviewBody?.call(context, item!, review);
        final moving =
            session.waiting.length > 1 ||
            (item != null && !session.isWaiting(item.key) && of > 0);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            PhoneHeader(
              leading: HeaderButton.back(onTap: _close),
              title: _Title(at: at, of: of, item: item),
              onTitleTap: () => _openQueue(context, session),
              titleLabel: at > 0 ? '$at of $of waiting' : 'Needs you',
              titleHint: 'Shows everything that waits',
              trailing: [
                if (moving) ...[
                  HeaderButton(
                    label: 'Previous',
                    glyph: (color) => ExtensionGlyph(
                      icon: 'chevron-up',
                      name: 'Previous',
                      color: color,
                      size: 22,
                    ),
                    onTap: () {
                      unawaited(Haptics.selection());
                      session.previous();
                    },
                  ),
                  HeaderButton(
                    label: 'Next',
                    glyph: (color) => ExtensionGlyph(
                      icon: 'chevron-down',
                      name: 'Next',
                      color: color,
                      size: 22,
                    ),
                    onTap: () {
                      unawaited(Haptics.selection());
                      session.next();
                    },
                  ),
                ],
              ],
            ),
            Expanded(
              child: item == null
                  ? _NothingLeft(session: session)
                  // The next card fades up as the last goes (§7.4); with
                  // Reduce Motion it simply swaps. Only one card is built.
                  : TweenAnimationBuilder<double>(
                      key: ValueKey(item.key),
                      tween: Tween(begin: 0, end: 1),
                      duration: MediaQuery.disableAnimationsOf(context)
                          ? Duration.zero
                          : Motion.base,
                      curve: Curves.easeOut,
                      builder: (context, shown, child) =>
                          Opacity(opacity: shown, child: child),
                      child: _Card(
                        session: session,
                        item: item,
                        own: own,
                        onContext: widget.onContext,
                      ),
                    ),
            ),
            if (session.receipt case final receipt?)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: Space.xs),
                child: Text(
                  receipt,
                  textAlign: TextAlign.center,
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textMuted),
                ),
              ),
            if (item != null && !(own?.answers ?? false))
              _ThumbDock(key: phoneThumbDockKey, session: session, item: item),
          ],
        );
      },
    );
  }
}

/// "3 of 7 ▾": where the card is among those waiting, a button that opens
/// the queue. "Needs you" with nothing waiting.
class _Title extends StatelessWidget {
  const _Title({required this.at, required this.of, required this.item});

  final int at;
  final int of;
  final NeedsYouItem? item;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final style = PhoneType.emphasis.copyWith(color: palette.text);
    if (of == 0 || at == 0) {
      return Text(of == 0 ? 'Needs you' : '$of waiting', style: style);
    }
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        // At the largest sizes "3/7" keeps the arrows in the bar.
        Flexible(
          child: Text(
            largeText(context) ? '$at/$of' : '$at of $of',
            style: style,
          ),
        ),
        const SizedBox(width: Space.xs),
        ExtensionGlyph(
          icon: 'chevron-down',
          name: '',
          color: palette.textMuted,
          size: 14,
        ),
      ],
    );
  }
}

/// The card: who asks, as a row that opens their conversation, then the
/// card itself with its own controls; how it ended, when it did while it
/// shows.
class _Card extends StatelessWidget {
  const _Card({
    required this.session,
    required this.item,
    this.own,
    this.onContext,
  });

  final FocusSession session;
  final NeedsYouItem item;

  /// The platform's own review of the card's file, if it has one.
  final FocusReviewBody? own;
  final VoidCallback? onContext;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final entries = session.entries;
    final outcome = session.outcomeOf(item.key);
    final own = this.own;
    if (own != null && own.answers) {
      return Listener(
        onPointerDown: (_) => session.touched(item.key),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _WhoAsks(session: session, item: item, onContext: onContext),
            Expanded(
              child: AnswerDock(
                answer: session.answer,
                draft: null,
                itemKey: item.key,
                child: own.body,
              ),
            ),
          ],
        ),
      );
    }
    return Listener(
      onPointerDown: (_) => session.touched(item.key),
      child: CustomScrollView(
        slivers: [
          SliverToBoxAdapter(
            child: _WhoAsks(session: session, item: item, onContext: onContext),
          ),
          SliverPadding(
            padding: const EdgeInsets.fromLTRB(
              Space.l,
              Space.m,
              Space.l,
              Space.l,
            ),
            sliver: SliverToBoxAdapter(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  // Taken back, or answered on another device, while it
                  // shows: it says so and never moves on by itself (§8).
                  if (outcome != null &&
                      outcome.state == FocusOutcomeState.ended) ...[
                    Row(
                      children: [
                        Expanded(
                          child: Text(
                            outcome.words,
                            style: type.label.copyWith(
                              color: palette.textMuted,
                            ),
                          ),
                        ),
                        Button(
                          label: 'Next',
                          primary: true,
                          onPressed: session.waiting.isEmpty
                              ? null
                              : session.next,
                        ),
                      ],
                    ),
                    const SizedBox(height: Space.m),
                  ],
                  if (own != null) ...[
                    SizedBox(height: 360, child: own.body),
                    const SizedBox(height: Space.m),
                  ],
                  AnswerDock(
                    answer: session.answer,
                    draft: null,
                    itemKey: item.key,
                    // Changed by its agent a moment ago: its own buttons
                    // take nothing until it's been seen (§8).
                    child: AbsorbPointer(
                      absorbing: session.guarded,
                      child: NeedsYouCard(
                        item: item,
                        entry: item.isEntry ? entries.entryOf(item) : null,
                        message: entries.messageOf(item),
                        failed: entries.failedOf(item),
                        workspace: session.backend.workspace.workspace,
                      ),
                    ),
                  ),
                  // Signing in again, unlocking the secrets, saving the
                  // recovery key: no card of its own, so its way on here,
                  // under the thumb's reach (a list's row does it on a tap).
                  if (item.isAction)
                    Button(
                      label: _actionLabel(item),
                      primary: true,
                      onPressed: () => _act(context, item),
                    ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// What an action item's button says.
String _actionLabel(NeedsYouItem item) => switch (item.kind) {
  NeedsYouKind.signIn => 'Sign in again',
  NeedsYouKind.vault => 'Unlock secrets…',
  _ => 'Save recovery key…',
};

/// Does what an action item asks, as Needs you's list does on a tap: signs
/// in again (Settings → Providers shows how it goes), or opens the form
/// that unlocks the secrets or saves the recovery key.
void _act(BuildContext context, NeedsYouItem item) {
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

/// Who asks: their character, name and role, the project and how long it
/// waited, tinted their color; a tap pushes the card's conversation
/// (Context on a phone, §7.6), and Back comes back to the card. A system
/// item has no conversation: its row says what waits on it.
class _WhoAsks extends StatelessWidget {
  const _WhoAsks({required this.session, required this.item, this.onContext});

  final FocusSession session;
  final NeedsYouItem item;
  final VoidCallback? onContext;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = session.backend;
    final workspace = backend.workspace.workspace;
    final agent = workspace.agents
        .where((agent) => agent.id == item.agentId)
        .firstOrNull;
    final project = agent == null
        ? null
        : workspace.projects
              .where((project) => project.id == agent.projectId)
              .firstOrNull;
    final pipe = agent == null || agent.pipeId.isEmpty
        ? null
        : workspace.pipes.where((pipe) => pipe.id == agent.pipeId).firstOrNull;
    final entries = session.entries;
    final (:who, :what) = needsYouWords(
      backend,
      workspace,
      item,
      entry: item.isEntry ? entries.entryOf(item) : null,
      message: entries.messageOf(item),
    );
    final time = entries.timeOf(item, workspace);
    final watch = focusWatchOf(item) != null;
    final routes = PhoneRoutes.of(context);
    final open =
        onContext ??
        (routes == null
            ? null
            : () => routes.push<void>(
                PhoneRoute(
                  id: 'needs-you:context:${item.key}',
                  builder: (context) =>
                      _ContextPage(session: session, who: who),
                ),
              ));
    final row = Container(
      constraints: const BoxConstraints(minHeight: Touch.target),
      padding: const EdgeInsets.fromLTRB(Space.l, Space.s, Space.m, Space.s),
      color: agent == null
          ? null
          : agentColor(agent.color).withValues(alpha: 0.06),
      child: Row(
        children: [
          if (agent != null) ...[
            AgentCharacter(agent: agent, size: 32),
            const SizedBox(width: Space.m),
          ],
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Builder(
                  builder: (context) {
                    // Its agent's role quietly beside its name, cut first;
                    // wrapping at the largest text (§3.10).
                    final named = needsYouWho(workspace, item, who);
                    return NameRoleLine(
                      name: named.name,
                      role: named.role,
                      after: [
                        if (named.after.isNotEmpty) TextSpan(text: named.after),
                      ],
                      wrap: largeText(context),
                      style: type.emphasis.copyWith(color: palette.text),
                      roleStyle: type.emphasis.copyWith(
                        color: palette.textMuted,
                        fontWeight: FontWeight.w400,
                      ),
                    );
                  },
                ),
                if (!watch)
                  Text(
                    what,
                    style: type.caption.copyWith(color: palette.textMuted),
                  )
                else if (project != null || pipe != null || time > 0)
                  Ticking(
                    active: time > 0,
                    builder: (context, now) => Text(
                      [
                        if (project != null) project.name,
                        if (pipe != null) pipe.name,
                        if (time > 0)
                          'waited ${formatAge(time, now).replaceFirst(' ago', '')}',
                      ].join(' · '),
                      style: type.caption.copyWith(
                        color: time > 0 && waitedADay(time, now)
                            ? palette.signal
                            : palette.textFaint,
                      ),
                    ),
                  ),
              ],
            ),
          ),
          if (watch && open != null)
            ExtensionGlyph(
              icon: 'chevron-right',
              name: '',
              color: palette.textFaint,
              size: 18,
            ),
        ],
      ),
    );
    // Whose card it is grows with the text as the bar does, only so far,
    // so the card keeps the screen at the largest sizes.
    final clamped = MediaQuery.withClampedTextScaling(
      maxScaleFactor: phoneTitleTextScale,
      child: row,
    );
    if (!watch || open == null) return clamped;
    return Semantics(
      button: true,
      label: '$who, ${agent?.role ?? ''}',
      hint: "Opens $who's conversation",
      excludeSemantics: true,
      child: Pressable(onTap: open, builder: (context, _) => clamped),
    );
  }
}

/// The card's conversation, pushed over the card: Context, as a phone
/// shows it (§7.6). It's watched, never selected, so the person's other
/// devices stay where they are; Back goes back to the card.
class _ContextPage extends StatelessWidget {
  const _ContextPage({required this.session, required this.who});

  final FocusSession session;
  final String who;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        PhoneHeader(
          leading: HeaderButton.back(
            onTap: () => PhoneRoutes.of(context)?.pop<void>(),
          ),
          title: Text(
            who,
            style: PhoneType.emphasis.copyWith(color: palette.text),
          ),
        ),
        Expanded(child: FocusContext(session: session)),
      ],
    );
  }
}

/// The answer at the screen's foot, over the keyboard, for a card that
/// leaves it to the dock ([CardAnswer.docked]): its field with the mic,
/// always open, then the thumb row, the no at the leading side and the
/// yes, filled and wider, under the thumb (§7.3). Words typed make the yes
/// Submit (#370). Other cards keep their own controls in the card.
class _ThumbDock extends StatefulWidget {
  const _ThumbDock({super.key, required this.session, required this.item});

  final FocusSession session;
  final NeedsYouItem item;

  @override
  State<_ThumbDock> createState() => _ThumbDockState();
}

class _ThumbDockState extends State<_ThumbDock> {
  final _focus = FocusNode(debugLabel: 'Needs you answer');

  FocusSession get _session => widget.session;

  @override
  void initState() {
    super.initState();
    _session.answer.addListener(_changed);
    _session.intoField = _focus.requestFocus;
  }

  @override
  void didUpdateWidget(_ThumbDock old) {
    super.didUpdateWidget(old);
    if (old.session != widget.session) {
      old.session.answer.removeListener(_changed);
      widget.session.answer.addListener(_changed);
    }
  }

  @override
  void dispose() {
    _session.answer.removeListener(_changed);
    if (_session.intoField == _focus.requestFocus) _session.intoField = null;
    _focus.dispose();
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  /// What didn't go in as files were added, said over the field.
  String? _filesNote;

  Future<void> _yes() async {
    unawaited(Haptics.light());
    await _session.yes();
  }

  /// The no: with words typed they go as its reason; with none, the quick
  /// reasons as an action sheet (§7.5), one tap each.
  Future<void> _no(CardAnswer answer) async {
    unawaited(Haptics.light());
    if (answer.typed) return _session.no(reason: null);
    const none = '';
    final reason = await showActions<String>(
      context,
      title: 'Why not?',
      message:
          'The reason goes to ${focusWho(_session.backend.workspace.workspace, widget.item)}.',
      actions: [
        for (final reason in focusDismissReasons) SheetAction(reason, reason),
        const SheetAction(focusDismissWithoutReason, none, destructive: true),
      ],
    );
    if (reason == null || !mounted) return;
    await _session.no(reason: reason == none ? null : reason, askWhy: false);
  }

  @override
  Widget build(BuildContext context) {
    final answer = _session.answer.value;
    final field = answer?.field;
    if (answer == null || !answer.docked || field == null) {
      return const SizedBox.shrink();
    }
    final palette = Palette.of(context);
    final who = focusWho(_session.backend.workspace.workspace, widget.item);
    final placeholder = who.isEmpty ? 'Your answer' : 'Your answer to $who';
    final answerable = _session.answerable;
    final files = answer.filesKey;
    return Container(
      decoration: BoxDecoration(
        color: palette.canvas,
        border: Border(top: BorderSide(color: palette.hairline)),
      ),
      padding: const EdgeInsets.fromLTRB(Space.l, Space.s, Space.l, Space.s),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // Files to go with the answer, as the card in its conversation
          // takes them (#511; Zucchero row 651).
          // The files themselves show on the card, under its words
          // (Zucchero 46973): here, only what didn't go in.
          if (files != null) ...[
            if (_filesNote case final note?)
              Padding(
                padding: const EdgeInsets.only(bottom: Space.xs),
                child: Text(
                  note,
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.danger),
                ),
              ),
          ],
          Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              Expanded(
                child: Dictation(
                  controller: field,
                  focusNode: _focus,
                  keep: DictationFor('focus/${widget.item.key}', placeholder),
                  child: TextField(
                    controller: field,
                    focusNode: _focus,
                    placeholder: placeholder,
                    minLines: 1,
                    maxLines: 5,
                    semanticsLabel: placeholder,
                  ),
                ),
              ),
              if (files != null)
                CardAttachButton(
                  card: files,
                  enabled: answerable && !answer.busy,
                  onNote: (note) {
                    if (mounted) setState(() => _filesNote = note);
                  },
                ),
            ],
          ),
          const SizedBox(height: Space.s),
          ListenableBuilder(
            listenable: field,
            builder: (context, _) {
              final typed = answer.typed;
              final yes = typed ? answer.onSubmit : answer.onYes;
              final yesButton = Button(
                // Changed a moment ago: it says so, and waits (§8).
                label: _session.guarded
                    ? 'Updated'
                    : typed
                    ? 'Submit'
                    : answer.yes,
                primary: true,
                onPressed: answerable && yes != null
                    ? () => unawaited(_yes())
                    : null,
              );
              // A card with no "no" (a usage limit's Continue) has none.
              final noButton = typed || answer.no.isEmpty
                  ? null
                  : Button(
                      label: answer.no,
                      onPressed: answerable && answer.onNo != null
                          ? () => unawaited(_no(answer))
                          : null,
                    );
              // At the accessibility sizes they stack, the yes at the
              // bottom, nearest the thumb.
              if (largeText(context)) {
                return Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    if (noButton != null) ...[
                      noButton,
                      const SizedBox(height: Space.s),
                    ],
                    yesButton,
                  ],
                );
              }
              return SizedBox(
                height: phoneThumbRow,
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    if (noButton != null) ...[
                      Expanded(child: noButton),
                      const SizedBox(width: Space.s),
                    ],
                    Expanded(flex: 2, child: yesButton),
                  ],
                ),
              );
            },
          ),
        ],
      ),
    );
  }
}

/// Nothing left: "Nothing needs you.", what the person did this sitting,
/// and the agents at work now. Back is the way out; it never closes by
/// itself.
class _NothingLeft extends StatelessWidget {
  const _NothingLeft({required this.session});

  final FocusSession session;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = session.backend.workspace.workspace;
    final working = [
      for (final agent in workspace.agents)
        if (agent.status == AgentStatus.AGENT_STATUS_WORKING) agent,
    ];
    final answered = session.answered;
    final minutes = DateTime.now().difference(session.startedAt).inMinutes;
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(Space.xl),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              'Nothing needs you.',
              textAlign: TextAlign.center,
              style: type.title.copyWith(color: palette.textMuted),
            ),
            if (answered > 0) ...[
              const SizedBox(height: Space.s),
              Text(
                'You answered $answered in '
                '${minutes < 1 ? 'under a minute' : '$minutes min'}.',
                textAlign: TextAlign.center,
                style: type.caption.copyWith(color: palette.textFaint),
              ),
            ],
            if (working.isNotEmpty) ...[
              const SizedBox(height: Space.l),
              Wrap(
                spacing: Space.s,
                runSpacing: Space.s,
                alignment: WrapAlignment.center,
                children: [
                  for (final agent in working)
                    Semantics(
                      label: agent.name,
                      child: AgentCharacter(agent: agent, size: 24, live: true),
                    ),
                ],
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// The queue, as a sheet from the title (§7.4): Waiting, every card of the
/// sitting in its order, the one showing checked and those answered here
/// saying how they ended; a tap shows that card. History, what was already
/// decided.
Future<void> _openQueue(BuildContext context, FocusSession session) {
  unawaited(Haptics.selection());
  return showPanel(
    context,
    title: 'Needs you',
    tall: true,
    builder: (context, close) => _QueueSheet(session: session, close: close),
  );
}

class _QueueSheet extends StatefulWidget {
  const _QueueSheet({required this.session, required this.close});

  final FocusSession session;
  final VoidCallback close;

  @override
  State<_QueueSheet> createState() => _QueueSheetState();
}

class _QueueSheetState extends State<_QueueSheet> {
  var _history = false;

  @override
  Widget build(BuildContext context) {
    final session = widget.session;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        NeedsYouTabs(
          history: _history,
          choose: (history) => setState(() => _history = history),
        ),
        const SizedBox(height: Space.s),
        Expanded(
          child: _history
              ? const NeedsYouHistoryList()
              : ListenableBuilder(
                  listenable: Listenable.merge([session, session.entries]),
                  builder: (context, _) {
                    final items = session.items;
                    if (items.isEmpty) {
                      return Center(
                        child: Text(
                          'Nothing needs you.',
                          style: TypeScale.of(context).body
                              .copyWith(color: Palette.of(context).textMuted),
                        ),
                      );
                    }
                    return ListView(
                      children: [
                        for (final item in items)
                          _QueueRow(
                            key: ValueKey(item.key),
                            session: session,
                            item: item,
                            onTap: () {
                              unawaited(Haptics.selection());
                              session.select(item.key);
                              widget.close();
                            },
                          ),
                      ],
                    );
                  },
                ),
        ),
      ],
    );
  }
}

/// A row of the queue sheet, 56pt: who asks over what, how long it waited
/// or how it ended; a check on the card showing.
class _QueueRow extends StatelessWidget {
  const _QueueRow({
    super.key,
    required this.session,
    required this.item,
    required this.onTap,
  });

  final FocusSession session;
  final NeedsYouItem item;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = session.backend;
    final workspace = backend.workspace.workspace;
    final entries = session.entries;
    final (:who, :what) = needsYouWords(
      backend,
      workspace,
      item,
      entry: item.isEntry ? entries.entryOf(item) : null,
      message: entries.messageOf(item),
    );
    final current = session.current?.key == item.key;
    final waiting = session.isWaiting(item.key);
    final outcome = session.outcomeOf(item.key);
    final time = entries.timeOf(item, workspace);
    final agent = workspace.agents
        .where((agent) => agent.id == item.agentId)
        .firstOrNull;
    final ending = outcome?.words;
    return Semantics(
      button: true,
      selected: current,
      label: [who, what, ?ending].join(', '),
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, pressed) => Container(
          constraints: const BoxConstraints(minHeight: 56),
          color: pressed ? palette.hover : null,
          padding: const EdgeInsets.symmetric(vertical: Space.xs),
          child: Row(
            children: [
              SizedBox.square(
                dimension: 28,
                child: Center(
                  child: agent != null
                      ? AgentCharacter(agent: agent, size: 28)
                      // Claktory's own (an alert of its own): its cog, as
                      // Needs you's alerts have it (Illuso 46868).
                      : item.kind == NeedsYouKind.alert
                      ? CogGlyph(color: palette.textMuted)
                      : ExtensionGlyph(
                          icon: _systemGlyph(item),
                          name: '',
                          color: palette.textMuted,
                        ),
                ),
              ),
              const SizedBox(width: Space.m),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    // Its agent's role beside its name, cut first (§3.10,
                    // Lucy 46817).
                    Builder(
                      builder: (context) {
                        final named = needsYouWho(workspace, item, who);
                        return NameRoleLine(
                          name: named.name,
                          role: named.role,
                          after: [
                            if (named.after.isNotEmpty)
                              TextSpan(text: named.after),
                          ],
                          style: type.label.copyWith(
                            color: waiting ? palette.text : palette.textMuted,
                            fontWeight: FontWeight.w600,
                          ),
                          roleStyle: type.label.copyWith(
                            color: palette.textMuted,
                            fontWeight: FontWeight.w400,
                          ),
                        );
                      },
                    ),
                    Text(
                      what,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: type.body.copyWith(
                        color: waiting ? palette.textMuted : palette.textFaint,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: Space.s),
              if (ending != null)
                Text(
                  ending,
                  style: type.caption.copyWith(color: palette.textFaint),
                )
              else if (time > 0)
                Ticking(
                  active: true,
                  builder: (context, now) => Text(
                    formatAge(time, now),
                    style: type.caption.copyWith(
                      color: waitedADay(time, now)
                          ? palette.signal
                          : palette.textFaint,
                    ),
                  ),
                ),
              if (current) ...[
                const SizedBox(width: Space.s),
                ExtensionGlyph(
                  icon: 'check',
                  name: 'Showing',
                  color: palette.signal,
                  size: 16,
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// A system item's glyph in the queue, as focus mode's on a Mac.
String _systemGlyph(NeedsYouItem item) => switch (item.kind) {
  NeedsYouKind.vault || NeedsYouKind.recoveryKey => 'key-round',
  NeedsYouKind.device => 'smartphone',
  NeedsYouKind.providerWait => 'chart-column',
  NeedsYouKind.keepAwake ||
  NeedsYouKind.backendService ||
  NeedsYouKind.lidStuck => 'laptop-minimal',
  NeedsYouKind.chat => 'message-circle',
  NeedsYouKind.signIn => 'user',
  _ => 'arrow-left-right',
};
