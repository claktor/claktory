// #343 U6: Needs you's item for the agents and chats waiting on one
// provider's usage limit or credit (Workspace.provider_waits), opened: the
// ways forward, all from the waiters themselves. Continue on another
// provider (for all of them, the count asked first), a usage reset the
// provider's accounts have, Try now, or Sign in… to another provider when
// there's none to carry on with.
import 'dart:async';

import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'identity.dart';
import 'mobile/lines.dart' show ControlsLine;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'usage.dart';
import 'usage_resets.dart';
import 'usage_wait.dart';

/// [workspace]'s wait on [providerId], while there is one.
ProviderWait? providerWaitOf(Workspace workspace, String providerId) => [
  for (final wait in workspace.providerWaits)
    if (wait.providerId == providerId) wait,
].firstOrNull;

/// Who waits first: an agent, else a chat; the one the ways forward are
/// read from and sent with.
String firstWaiter(ProviderWait wait) =>
    wait.agentIds.firstOrNull ?? wait.chatIds.firstOrNull ?? '';

/// [wait]'s provider's accounts' circles, for its glyph and its resets.
List<UsageMeter> providerWaitMeters(Workspace workspace, ProviderWait wait) => [
  for (final meter in usageMeters(workspace.providers))
    if (meter.provider.id == wait.providerId) meter,
];

/// The opened item: what carrying on elsewhere would change, and the ways
/// forward.
class ProviderWaitCard extends StatefulWidget {
  const ProviderWaitCard({super.key, required this.wait});

  final ProviderWait wait;

  @override
  State<ProviderWaitCard> createState() => _ProviderWaitCardState();
}

class _ProviderWaitCardState extends State<ProviderWaitCard> {
  var _busy = false;
  String? _error;

  Future<void> _continue(String waiting, UsageWaitOptions options) async {
    setState(() => _error = null);
    final refused = await continueOnAlternative(
      context,
      waiting: waiting,
      options: options,
      allOnly: true,
      // Busy once it's asked for, not while the person chooses.
      onSending: () {
        if (mounted) setState(() => _busy = true);
      },
    );
    if (!mounted) return;
    setState(() {
      _busy = false;
      _error = refused;
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final caption = TextStyles.caption;
    final workspace = AppScope.of(context).workspace.workspace;
    final wait = widget.wait;
    final waiting = firstWaiter(wait);
    final options = usageWaitOptions(context, waiting);
    final alternative = options?.alternative;
    final count = wait.agentIds.length + wait.chatIds.length;
    // What carrying on would change is the first waiter's: said only when
    // it's the one waiting.
    final detail = options == null
        ? null
        : alternative != null && count > 1
        ? null
        : usageWaitDetail(options);
    final resets = [
      for (final meter in providerWaitMeters(workspace, wait))
        for (final reset in resetsLeft(meter))
          if (reset.usable) (meter: meter, reset: reset),
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (detail != null) ...[
          Text(detail, style: caption.copyWith(color: palette.textMuted)),
          const SizedBox(height: Space.s),
        ],
        if (_error case final error?) ...[
          Text(error, style: caption.copyWith(color: palette.danger)),
          const SizedBox(height: Space.s),
        ],
        Align(
          alignment: AlignmentDirectional.centerStart,
          child: ControlsLine(
            children: [
              if (alternative != null)
                Button(
                  label: 'Continue on ${alternative.providerName}',
                  primary: true,
                  onPressed: _busy
                      ? null
                      : () => unawaited(_continue(waiting, options!)),
                ),
              for (final (:meter, :reset) in resets)
                Button(
                  label: 'Use ${inSentence(reset.label)}…',
                  onPressed: () => unawaited(
                    useResetDialog(context, meter: meter, reset: reset),
                  ),
                ),
              if (waiting.isNotEmpty) TryNow(wait: '', waiting: waiting),
              if (options?.noOtherProvider == true)
                Button(
                  label: 'Sign in…',
                  onPressed: () => AppScope.openProviders(context),
                ),
            ],
          ),
        ),
      ],
    );
  }
}
