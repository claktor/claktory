import 'dart:async' show unawaited;

import 'package:flutter/widgets.dart';

import 'backend.dart' show RequestFailure, fireAndLog;
import 'controls.dart' show Button, Toggle, ToggleTap;
import 'identity.dart';
import 'mobile/haptics.dart';
import 'mobile/lines.dart' show ActionLine, ControlsLine;
import 'mobile/metrics.dart' show TypeScale;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'usage.dart' show UsageMeter, formatUntil, usageMeters;
import 'usage_resets.dart' show resetsLeft, useResetDialog;

/// Whether [agent] is at work now: its run is open and not held up by its
/// accounts' usage limits (then it sleeps, and says what it waits for).
bool atWork(Agent agent) =>
    agent.status == AgentStatus.AGENT_STATUS_WORKING && agent.usageWait.isEmpty;

/// What [agent] waits for while usage limits hold it up (Agent.usage_wait),
/// with the time it asks again in this device's own time zone: "Waiting for
/// your Claude usage to reset at 18:57" today, "… at Fri 18:57" later in the
/// week. The backend writes its own local time, which may be another zone's.
/// As the backend says it when there's no time (it waits for the person);
/// empty when it isn't held up.
String usageWaitLine(Agent agent, {DateTime? now}) =>
    _waitLine(agent.usageWait, agent.usageWaitUntilMs.toInt(), now);

/// A chat's [usageWaitLine] (Chat.usage_wait).
String chatUsageWaitLine(Chat chat, {DateTime? now}) =>
    _waitLine(chat.usageWait, chat.usageWaitUntilMs.toInt(), now);

/// Whether [chat] is replying now: as [atWork], not while it waits out its
/// provider's usage.
bool chatAtWork(Chat chat) =>
    chat.status == AgentStatus.AGENT_STATUS_WORKING && chat.usageWait.isEmpty;

String _waitLine(String text, int ms, DateTime? now) {
  final at = text.lastIndexOf(' at ');
  if (text.isEmpty || ms <= 0 || at < 0) return text;
  return '${text.substring(0, at)} at '
      '${formatUntil(ms, now ?? DateTime.now())}';
}

/// Beside what an agent or chat waits for ([wait], its [usageWaitLine]):
/// Try now, to ask again at once (after upgrading a plan, say: its
/// provider reads its accounts' usage again), or Sign in… while it waits
/// for a sign-in, since asking again does nothing until then. [waiting] is
/// the agent's or chat's id.
class TryNow extends StatelessWidget {
  const TryNow({super.key, required this.wait, required this.waiting});

  final String wait;
  final String waiting;

  @override
  Widget build(BuildContext context) {
    if (wait.startsWith('Waiting for you to sign in to')) {
      return Button(
        label: 'Sign in…',
        onPressed: () => AppScope.openProviders(context),
      );
    }
    return Button(
      label: 'Try now',
      onPressed: () {
        if (isPhoneLayout(context)) unawaited(Haptics.light());
        // A second tap after the wait ended is refused: its line has gone
        // already, so there's nothing to say.
        fireAndLog(AppScope.of(context).tryAgainNow(waiting));
      },
    );
  }
}

/// What an agent or chat held up by its provider's usage can do about it
/// (#341), as the backend says: carry on with another signed-in provider
/// now ([alternative]), sign in to one when it has none ([noOtherProvider]),
/// or, once it carried on elsewhere, go back ([switched]).
typedef UsageWaitOptions = ({
  /// "Kai", or "this chat".
  String name,

  /// Its model, "<provider>/<model id>".
  String model,
  UsageWaitAlternative? alternative,
  bool noOtherProvider,
  UsageSwitch? switched,
});

/// The choice for all [count] of them at once: "Both", "All 3".
String usageWaitAll(int count) => count == 2 ? 'Both' : 'All $count';

/// [UsageWaitOptions] of the agent or chat [id]; null for one this app
/// doesn't hold.
UsageWaitOptions? usageWaitOptions(BuildContext context, String id) {
  final workspace = AppScope.of(context).workspace.workspace;
  for (final agent in workspace.agents) {
    if (agent.id != id) continue;
    return (
      name: agent.name,
      model: agent.model,
      alternative: agent.hasUsageWaitAlternative()
          ? agent.usageWaitAlternative
          : null,
      noOtherProvider: agent.usageWaitNoOtherProvider,
      switched: agent.hasUsageSwitch() ? agent.usageSwitch : null,
    );
  }
  for (final chat in workspace.chats) {
    if (chat.id != id) continue;
    return (
      name: 'this chat',
      model: chat.model,
      alternative: chat.hasUsageWaitAlternative()
          ? chat.usageWaitAlternative
          : null,
      noOtherProvider: chat.usageWaitNoOtherProvider,
      switched: chat.hasUsageSwitch() ? chat.usageSwitch : null,
    );
  }
  return null;
}

/// Whether [wait] (a [usageWaitLine]) waits for the network to come back
/// after a connection error (#364 O2), not for a usage limit: the backend
/// says it in usage_wait too, with nothing to switch or reset.
bool networkWait(String wait) => wait.startsWith('Waiting for the network');

/// A usage reset an account has left, with the account's meter.
typedef WaitReset = ({UsageMeter meter, UsageReset reset});

/// The usage resets that would refill a limit [model] (as Agent.model)
/// waits on (Squalo's U7): those of its provider's accounts with a
/// limit reached that the model draws on, refilled by the reset (one that
/// doesn't say which limits refills them all). As the backend counts
/// Anthropic's limits: a Fable model draws on the 5-hour, weekly and
/// Fable limits, every other model on all but Fable's.
List<WaitReset> waitResets(Workspace workspace, String model) {
  final slash = model.indexOf('/');
  if (slash < 0) return const [];
  final providerId = model.substring(0, slash);
  final fable =
      providerId == 'anthropic' &&
      model.substring(slash + 1).startsWith('claude-fable');
  bool drawsOn(String window) => fable
      ? const {'five_hour', 'weekly', 'weekly_fable'}.contains(window)
      : window != 'weekly_fable';
  return [
    for (final meter in usageMeters(workspace.providers))
      if (meter.provider.id == providerId)
        if (meter.usage case final usage?)
          for (final reset in resetsLeft(meter))
            if (usage.windows.any(
              (window) =>
                  window.usedPercent >= 100 &&
                  drawsOn(window.id) &&
                  (reset.windowIds.isEmpty ||
                      reset.windowIds.contains(window.id)),
            ))
              (meter: meter, reset: reset),
  ];
}

/// Use a reset… on the wait line: with one reset that would help, asks
/// before using it; with several, which one first ("Fable reset ·
/// matt@work.test").
Future<void> _useReset(BuildContext context, List<WaitReset> resets) async {
  WaitReset? chosen = resets.length == 1 ? resets.single : null;
  if (resets.length > 1) {
    chosen = await showActions<WaitReset>(
      context,
      title: 'Use which reset?',
      actions: [
        for (final option in resets)
          SheetAction(
            option.meter.who.isEmpty
                ? option.reset.label
                : '${option.reset.label} · ${option.meter.who}',
            option,
          ),
      ],
    );
  }
  if (chosen == null || !context.mounted) return;
  await useResetDialog(context, meter: chosen.meter, reset: chosen.reset);
}

/// Under what it waits for, what carrying on elsewhere would change: the
/// model, when it's the closest rather than the same, and the effort, when
/// that model hasn't its own. Or, with no other provider signed in, that
/// one would let it carry on. Null when there's nothing to say.
String? usageWaitDetail(UsageWaitOptions options) {
  final alternative = options.alternative;
  if (alternative != null) {
    final effort = alternative.effortChanged && alternative.effort.isNotEmpty
        ? alternative.effort
        : null;
    if (!alternative.sameModel) {
      return 'On ${alternative.providerName}, it uses '
          '${alternative.modelName} instead'
          '${effort == null ? '' : ', at $effort effort'}.';
    }
    if (effort != null) {
      return 'On ${alternative.providerName}, it runs at $effort effort.';
    }
    return null;
  }
  if (options.noOtherProvider) {
    return 'Sign in to another provider, such as OpenRouter, to carry on '
        'now.';
  }
  return null;
}

/// Continue on another provider (#341) for the agent or chat [waiting]
/// (its id), with [options] its [usageWaitOptions]: with others waiting on
/// the same limit, asks first whether only it goes or all of them (with
/// [allOnly], as Needs you asks for a provider's waiters, only whether all
/// go: "All 3", or "Continue" for one), saying the other provider charges
/// for it; then sends it, [onSending] just before. The count shown goes
/// with all (expected_others), so should more or fewer wait by the tap,
/// none moves (Zucchero's row 223). Completes with the refusal in the
/// backend's words, or null once it's sent or the person cancelled.
Future<String?> continueOnAlternative(
  BuildContext context, {
  required String waiting,
  required UsageWaitOptions options,
  bool allOnly = false,
  VoidCallback? onSending,
}) async {
  final alternative = options.alternative!;
  final provider = alternative.providerName;
  final others = alternative.othersWaiting;
  var all = false;
  if (others > 0 || allOnly) {
    final chosen = await showActions<bool>(
      context,
      title: 'Continue on $provider?',
      message: '$provider charges you for what they use there.',
      actions: [
        if (!allOnly) SheetAction('Only ${options.name}', false),
        if (others > 0)
          SheetAction(usageWaitAll(others + 1), true)
        else
          const SheetAction('Continue', false),
      ],
    );
    if (chosen == null || !context.mounted) return null;
    all = chosen;
  } else if (isPhoneLayout(context)) {
    unawaited(Haptics.light());
  }
  onSending?.call();
  try {
    await AppScope.of(context).continueOnAlternative(
      waiting,
      allWaiting: all,
      expectedOthers: all ? others : 0,
    );
    return null;
  } on RequestFailure catch (failure) {
    return failure.message;
  }
}

/// The line above the message box while an agent or chat ([waiting], its
/// id) waits out its provider's usage ([wait], its [usageWaitLine]): what
/// it waits for, what carrying on elsewhere would change (or a refusal),
/// and its controls at the end: Continue on another provider (#341), Try
/// now (or Sign in…), Sign in… to another provider when none is, and
/// [stop]. Buttons under the words where they don't fit beside them.
class UsageWaitActionLine extends StatefulWidget {
  const UsageWaitActionLine({
    super.key,
    required this.wait,
    required this.waiting,
    required this.stop,
  });

  final String wait;
  final String waiting;
  final Widget stop;

  @override
  State<UsageWaitActionLine> createState() => _UsageWaitActionLineState();
}

class _UsageWaitActionLineState extends State<UsageWaitActionLine> {
  var _busy = false;
  String? _error;

  /// The words, a refusal among them, and what they said when last
  /// brought into view.
  final _words = GlobalKey();
  final _refusal = GlobalKey();
  String? _shown;

  /// On a phone the controls under the conversation scroll, kept at the
  /// message box: at the largest text on a small one, the words above the
  /// buttons were out of view (Zucchero's row 228). Brings them into view
  /// each time they change (a new wait, a refusal) or their size does (the
  /// text size or the display's changed while it shows, Sale's row 66), not
  /// on every build, so the person's own scrolling stays; a refusal first,
  /// where both don't fit, since it says why their tap did nothing.
  void _reveal(String said) {
    if (said == _shown) return;
    _shown = said;
    _revealAfterLayout();
  }

  /// Brings the words into view once this frame is laid out.
  void _revealAfterLayout() {
    if (!mounted || !isPhoneLayout(context)) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final words = _refusal.currentContext ?? _words.currentContext;
      if (words == null || !words.mounted) return;
      unawaited(
        Scrollable.ensureVisible(
          words,
          // The scroll runs up from the box: its far end is the top.
          alignment: 1,
        ),
      );
    });
  }

  @override
  void didUpdateWidget(UsageWaitActionLine old) {
    super.didUpdateWidget(old);
    if (old.waiting != widget.waiting) _error = null;
  }

  Future<void> _continue(UsageWaitOptions options) async {
    final refusal = await continueOnAlternative(
      context,
      waiting: widget.waiting,
      options: options,
      onSending: () => setState(() {
        _busy = true;
        _error = null;
      }),
    );
    if (!mounted) return;
    setState(() {
      _busy = false;
      _error = refusal;
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final caption = TypeScale.of(context).caption;
    final options = usageWaitOptions(context, widget.waiting);
    final alternative = options?.alternative;
    final detail = options == null ? null : usageWaitDetail(options);
    final signingIn = widget.wait.startsWith('Waiting for you to sign in to');
    // A wait for the network (O2) isn't on a limit: no reset refills it.
    final resets = options == null || networkWait(widget.wait)
        ? const <WaitReset>[]
        : waitResets(AppScope.of(context).workspace.workspace, options.model);
    _reveal(
      '${widget.wait}\n$detail\n$_error\n'
      // Its comparable value: Android's own scaling (nonlinear) leaves a
      // big size as it is, so a size scaled can't tell (Sale's row 73).
      // ignore: deprecated_member_use
      '${MediaQuery.textScalerOf(context).textScaleFactor} '
      '${MediaQuery.sizeOf(context)}',
    );
    final words = Column(
      key: _words,
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        Text(
          widget.wait,
          textAlign: TextAlign.end,
          style: caption.copyWith(color: palette.textMuted),
        ),
        if (detail != null)
          Text(
            detail,
            textAlign: TextAlign.end,
            style: caption.copyWith(color: palette.textFaint),
          ),
        if (_error case final error?)
          Text(
            error,
            key: _refusal,
            textAlign: TextAlign.end,
            style: caption.copyWith(color: palette.danger),
          ),
      ],
    );
    return ActionLine(
      // Their size changing (the text size, on Android in steps, at a
      // display size that isn't the default: Sale's row 73) brings them
      // into view again, whatever else changed or didn't.
      lead: NotificationListener<SizeChangedLayoutNotification>(
        onNotification: (_) {
          _revealAfterLayout();
          return true;
        },
        child: SizeChangedLayoutNotifier(child: words),
      ),
      // Each under the one before where even the whole line can't hold
      // them (a small phone at the largest text).
      action: ControlsLine(
        children: [
          // Free, so before paying another provider.
          if (resets.isNotEmpty)
            Button(
              label: 'Use a reset…',
              onPressed: () => unawaited(_useReset(context, resets)),
            ),
          if (alternative != null)
            Button(
              label: 'Continue on ${alternative.providerName}',
              onPressed: _busy ? null : () => _continue(options!),
            ),
          TryNow(wait: widget.wait, waiting: widget.waiting),
          if (options?.noOtherProvider == true && !signingIn)
            Button(
              label: 'Sign in…',
              onPressed: () => AppScope.openProviders(context),
            ),
          widget.stop,
        ],
      ),
    );
  }
}

/// Above the message box while an agent or chat ([id]) carries on with
/// another provider (Agent.usage_switch): once its own provider has room,
/// "Your Claude usage has room again." with Back to Claude (all that are
/// ready, or only it, when others are); before that, when it goes back
/// (friend bug #8 A1): "Goes back to Claude at Fri 18:00", with Back now.
/// One that went back by itself and hit the limit again soon after
/// (UsageSwitch.goes_back false) stays until that limit resets, and goes
/// back then: the same line, Back now the only way back sooner (Abbacchio,
/// Mista).
class UsageSwitchLine extends StatefulWidget {
  const UsageSwitchLine({super.key, required this.id});

  final String id;

  @override
  State<UsageSwitchLine> createState() => _UsageSwitchLineState();
}

class _UsageSwitchLineState extends State<UsageSwitchLine> {
  var _busy = false;
  String? _error;

  Future<void> _back(UsageWaitOptions options) async {
    final switched = options.switched!;
    var all = false;
    // As in Continue: only the count shown goes back.
    final others = switched.othersReady;
    if (others > 0) {
      final chosen = await showActions<bool>(
        context,
        title: 'Back to ${switched.providerName}?',
        actions: [
          SheetAction('Only ${options.name}', false),
          SheetAction(usageWaitAll(others + 1), true),
        ],
      );
      if (chosen == null || !mounted) return;
      all = chosen;
    } else if (isPhoneLayout(context)) {
      unawaited(Haptics.light());
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context)
          .switchBack(widget.id, all: all, expectedOthers: all ? others : 0);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final options = usageWaitOptions(context, widget.id);
    final switched = options?.switched;
    if (options == null || switched == null) {
      return const SizedBox.shrink();
    }
    final provider = switched.providerName;
    final at = switched.goesBackAtMs.toInt();
    final said = switched.ready
        ? 'Your $provider usage has room again.'
        : at > 0
        ? 'Goes back to $provider at ${formatUntil(at, DateTime.now())}'
        : 'Goes back to $provider once your usage has room';
    final palette = Palette.of(context);
    final caption = TypeScale.of(context).caption;
    final phone = isPhoneLayout(context);
    return Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: contentMaxWidth),
        child: Padding(
          // As the working line above the box: its end lines up with the
          // box's.
          padding: EdgeInsets.fromLTRB(
            phone ? Space.s : Space.xl,
            Space.s,
            phone ? Space.s : Space.xl,
            0,
          ),
          child: ActionLine(
            lead: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                Text(
                  said,
                  textAlign: TextAlign.end,
                  style: caption.copyWith(color: palette.textMuted),
                ),
                if (_error case final error?)
                  Text(
                    error,
                    textAlign: TextAlign.end,
                    style: caption.copyWith(color: palette.danger),
                  ),
              ],
            ),
            action: Button(
              // Before it has room, back now: it then waits there.
              label: switched.ready ? 'Back to $provider' : 'Back now',
              onPressed: _busy ? null : () => _back(options),
            ),
          ),
        ),
      ),
    );
  }
}

/// "Kai is", "3 agents are", "A chat is", "2 agents and a chat are": who
/// waits, for the offer in Settings → Providers.
String _waitingSubject(List<Agent> agents, List<Chat> chats) {
  final who = <String>[
    if (agents.length == 1) agents.single.name,
    if (agents.length > 1) '${agents.length} agents',
    if (chats.length == 1) agents.isEmpty ? 'A chat' : 'a chat',
    if (chats.length > 1) '${chats.length} chats',
  ];
  final one = agents.length + chats.length == 1;
  return '${who.join(' and ')} ${one ? 'is' : 'are'}';
}

/// In Settings → Providers, on a signed-in [provider]'s card (Squalo's U2):
/// while agents or chats wait on another provider's usage and would carry
/// on with this one (their Agent.usage_wait_alternative names it), how many
/// wait and for which provider, what would change, and Continue on
/// [provider] for all of them, as the wait line's All N (#341). Nothing
/// when none would.
class UsageWaitOffer extends StatefulWidget {
  const UsageWaitOffer({super.key, required this.provider});

  final Provider provider;

  @override
  State<UsageWaitOffer> createState() => _UsageWaitOfferState();
}

class _UsageWaitOfferState extends State<UsageWaitOffer> {
  var _busy = false;
  String? _error;

  /// [groups]: those waiting, by the provider they wait on. One request for
  /// each provider, for all its waiters at once (all_waiting), as the wait
  /// line's All N: moving one wakes the others for a moment, so a request
  /// each was refused as not waiting (Abbacchio's rows 692-694). The count
  /// that goes with it is the backend's own (others_waiting), so the two
  /// agree; should more or fewer wait by the tap, none moves and the
  /// backend says so. One with a better alternative elsewhere (an OpenAI
  /// key) goes there, as with All N.
  Future<void> _continue(
    List<List<(String, UsageWaitAlternative)>> groups,
  ) async {
    if (isPhoneLayout(context)) unawaited(Haptics.light());
    setState(() {
      _busy = true;
      _error = null;
    });
    final backend = AppScope.of(context);
    String? refusal;
    for (final waiters in groups) {
      final (id, alternative) = waiters.first;
      final others = alternative.othersWaiting;
      try {
        await backend.continueOnAlternative(
          id,
          allWaiting: others > 0,
          expectedOthers: others,
        );
      } on RequestFailure catch (failure) {
        refusal ??= failure.message;
      }
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _error = refusal;
    });
  }

  @override
  Widget build(BuildContext context) {
    final provider = widget.provider;
    final workspace = AppScope.of(context).workspace.workspace;
    bool here(UsageWaitAlternative alternative) =>
        alternative.providerName == provider.name;
    final agents = [
      for (final agent in workspace.agents)
        if (agent.usageWait.isNotEmpty &&
            agent.hasUsageWaitAlternative() &&
            here(agent.usageWaitAlternative))
          agent,
    ];
    final chats = [
      for (final chat in workspace.chats)
        if (chat.usageWait.isNotEmpty &&
            chat.hasUsageWaitAlternative() &&
            here(chat.usageWaitAlternative))
          chat,
    ];
    if (agents.isEmpty && chats.isEmpty) return const SizedBox.shrink();
    // Which provider they wait for, from their models' "<provider>/…".
    final waitedOn = {
      for (final model in [
        for (final agent in agents) agent.model,
        for (final chat in chats) chat.model,
      ])
        model.split('/').first,
    };
    // Named as the wait names it ("your Claude usage"): a subscription's
    // product, else the provider.
    var waitsFor = 'their usage to reset';
    if (waitedOn.length == 1) {
      for (final other in workspace.providers) {
        if (other.id != waitedOn.single) continue;
        waitsFor = switch (other.id) {
          'anthropic' => 'Claude',
          'openai-codex' => 'ChatGPT',
          _ => other.name,
        };
      }
    }
    final alternatives = [
      for (final agent in agents) agent.usageWaitAlternative,
      for (final chat in chats) chat.usageWaitAlternative,
    ];
    final one = alternatives.length == 1;
    final instead = {
      for (final alternative in alternatives)
        if (!alternative.sameModel) alternative.modelName,
    };
    final name = provider.name;
    final changes = instead.isEmpty
        ? ''
        : instead.length == 1 && alternatives.every((a) => !a.sameModel)
        ? ' On $name, ${one ? 'it uses' : 'they use'} ${instead.single} '
              'instead.'
        : ' On $name, some use another model.';
    final groups = <String, List<(String, UsageWaitAlternative)>>{};
    for (final (id, model, alternative) in [
      for (final agent in agents)
        (agent.id, agent.model, agent.usageWaitAlternative),
      for (final chat in chats)
        (chat.id, chat.model, chat.usageWaitAlternative),
    ]) {
      groups.putIfAbsent(model.split('/').first, () => []).add((
        id,
        alternative,
      ));
    }
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: Space.m),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '${_waitingSubject(agents, chats)} waiting for $waitsFor.',
            style: type.body.copyWith(color: palette.text),
          ),
          const SizedBox(height: 2),
          Text(
            '$name charges you for what ${one ? 'it uses' : 'they use'} '
            'there.$changes',
            style: type.caption.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.s),
          Wrap(
            children: [
              Button(
                label: one ? 'Continue on $name' : 'Continue all on $name',
                primary: true,
                onPressed: _busy
                    ? null
                    : () => _continue(groups.values.toList()),
              ),
            ],
          ),
          if (_error case final error?)
            Padding(
              padding: const EdgeInsets.only(top: Space.xs),
              child: Text(
                error,
                style: type.caption.copyWith(color: palette.danger),
              ),
            ),
        ],
      ),
    );
  }
}

/// In Settings → Providers, under the providers: Continue automatically
/// (Workspace.usage_auto_continue, friend bug #8 A2), off unless the person
/// turns it on, saying it spends on the other provider without asking.
class UsageAutoContinueSetting extends StatefulWidget {
  const UsageAutoContinueSetting({super.key, required this.on});

  final bool on;

  @override
  State<UsageAutoContinueSetting> createState() =>
      _UsageAutoContinueSettingState();
}

class _UsageAutoContinueSettingState extends State<UsageAutoContinueSetting> {
  var _busy = false;
  String? _error;

  Future<void> _set(bool on) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).setUsageAutoContinue(on);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    const label = 'Continue automatically';
    final onChanged = _busy ? null : _set;
    return ToggleTap(
      value: widget.on,
      onChanged: onChanged,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 2),
            child: Toggle(value: widget.on, label: label, onChanged: onChanged),
          ),
          const SizedBox(width: Space.m),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(label, style: type.body.copyWith(color: palette.text)),
                Text(
                  'When an agent or chat reaches a Claude or ChatGPT usage '
                  "limit, it carries on at once with another provider you're "
                  'signed in to, such as OpenRouter, on the same model or the '
                  'closest one. That provider charges you without asking '
                  'first. It goes back by itself once your usage has '
                  'room.',
                  style: type.caption.copyWith(color: palette.textMuted),
                ),
                if (_error case final error?)
                  Text(
                    error,
                    style: type.caption.copyWith(color: palette.danger),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
