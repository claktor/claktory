import 'dart:async' show unawaited;

import 'package:flutter/widgets.dart';

import 'backend.dart' show RequestFailure, fireAndLog;
import 'controls.dart' show Button;
import 'identity.dart';
import 'mobile/haptics.dart';
import 'mobile/lines.dart' show ActionLine, ControlsLine;
import 'mobile/metrics.dart' show TypeScale;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'usage.dart' show formatUntil;

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
String usageWaitLine(Agent agent, {DateTime? now}) {
  final text = agent.usageWait;
  final ms = agent.usageWaitUntilMs.toInt();
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
  UsageWaitAlternative? alternative,
  bool noOtherProvider,
  UsageSwitch? switched,
});

/// The choice for all [count] of them at once: "Both", "All 3".
String _all(int count) => count == 2 ? 'Both' : 'All $count';

/// [UsageWaitOptions] of the agent or chat [id]; null for one this app
/// doesn't hold.
UsageWaitOptions? usageWaitOptions(BuildContext context, String id) {
  final workspace = AppScope.of(context).workspace.workspace;
  for (final agent in workspace.agents) {
    if (agent.id != id) continue;
    return (
      name: agent.name,
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
      alternative: chat.hasUsageWaitAlternative()
          ? chat.usageWaitAlternative
          : null,
      noOtherProvider: chat.usageWaitNoOtherProvider,
      switched: chat.hasUsageSwitch() ? chat.usageSwitch : null,
    );
  }
  return null;
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

  @override
  void didUpdateWidget(UsageWaitActionLine old) {
    super.didUpdateWidget(old);
    if (old.waiting != widget.waiting) _error = null;
  }

  Future<void> _continue(UsageWaitOptions options) async {
    final alternative = options.alternative!;
    final phone = isPhoneLayout(context);
    var all = false;
    // As many others as the choice counts: should more (or fewer) be
    // waiting by the tap, none moves and the backend says so (Zucchero's
    // row 223).
    final others = alternative.othersWaiting;
    if (others > 0) {
      // The others waiting on the same limit can go too: which is asked.
      final chosen = await showActions<bool>(
        context,
        title: 'Continue on ${alternative.providerName}?',
        message:
            '${alternative.providerName} charges you for what they use '
            'there.',
        actions: [
          SheetAction('Only ${options.name}', false),
          SheetAction(_all(others + 1), true),
        ],
      );
      if (chosen == null || !mounted) return;
      all = chosen;
    } else if (phone) {
      unawaited(Haptics.light());
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).continueOnAlternative(
        widget.waiting,
        allWaiting: all,
        expectedOthers: all ? others : 0,
      );
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final caption = TypeScale.of(context).caption;
    final options = usageWaitOptions(context, widget.waiting);
    final alternative = options?.alternative;
    final detail = options == null ? null : usageWaitDetail(options);
    final signingIn = widget.wait.startsWith('Waiting for you to sign in to');
    final words = Column(
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
            textAlign: TextAlign.end,
            style: caption.copyWith(color: palette.danger),
          ),
      ],
    );
    return ActionLine(
      lead: words,
      // Each under the one before where even the whole line can't hold
      // them (a small phone at the largest text).
      action: ControlsLine(
        children: [
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

/// Above the message box once an agent or chat ([id]) that carried on with
/// another provider (Agent.usage_switch) can go back, its own provider
/// having room again: "Your Claude usage has room again." with Back to
/// Claude (all
/// that are ready, or only it, when others are). Nothing otherwise.
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
          SheetAction(_all(others + 1), true),
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
    if (options == null || switched == null || !switched.ready) {
      return const SizedBox.shrink();
    }
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
                  'Your ${switched.providerName} usage has room again.',
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
              label: 'Back to ${switched.providerName}',
              onPressed: _busy ? null : () => _back(options),
            ),
          ),
        ),
      ),
    );
  }
}
