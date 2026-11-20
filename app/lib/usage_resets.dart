import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import 'backend.dart';
import 'controls.dart';
import 'forms.dart' show FormFrame;
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'usage.dart';

/// Usage resets: what a subscription can use to refill a limit early (a
/// Claude "5-hour reset", a ChatGPT reset credit), read with its usage
/// (`ProviderUsage.resets`). The usage circle gets a cue only when an
/// account is near a limit one of them refills; its card lists them all,
/// and using one always asks first and is the person's call: nothing uses
/// one on its own.

/// How full a limit is before its circle offers a reset.
const resetCuePercent = 90.0;

/// The resets [meter]'s account has left.
List<UsageReset> resetsLeft(UsageMeter meter) => [
  for (final reset in meter.usage?.resets ?? const <UsageReset>[])
    if (reset.left > 0) reset,
];

/// Whether [meter]'s circle shows the cue: a limit one of its resets
/// refills is at least [resetCuePercent] used (or reached). A reset that
/// doesn't say which limits it refills refills them all.
bool resetCue(UsageMeter meter) {
  final usage = meter.usage;
  if (usage == null) return false;
  for (final reset in resetsLeft(meter)) {
    for (final window in usage.windows) {
      final refills =
          reset.windowIds.isEmpty || reset.windowIds.contains(window.id);
      if (refills && window.usedPercent >= resetCuePercent) return true;
    }
  }
  return false;
}

/// "2 left", "2 of 3 left", and when they must be used by.
String resetCount(UsageReset reset, DateTime now) {
  final left = reset.total > 0
      ? '${reset.left} of ${reset.total} left'
      : '${reset.left} left';
  final expires = reset.expiresAt.toInt();
  return expires > 0 ? '$left, use by ${formatUntil(expires, now)}' : left;
}

/// The end of a usage card: the account's resets, each with Use…. Nothing
/// when it has none.
class UsageResets extends StatelessWidget {
  const UsageResets({super.key, required this.meter, this.now});

  final UsageMeter meter;

  /// The time expiries are counted from; null for now.
  final DateTime? now;

  @override
  Widget build(BuildContext context) {
    final resets = resetsLeft(meter);
    if (resets.isEmpty) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final now = this.now ?? DateTime.now();
    final caption = TextStyles.caption.copyWith(color: palette.textMuted);
    return Padding(
      padding: const EdgeInsets.only(top: Space.m),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Usage resets',
            style: TextStyles.label.copyWith(
              color: palette.text,
              fontWeight: FontWeight.w600,
            ),
          ),
          for (final reset in resets)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Flexible(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(reset.label),
                        Text(resetCount(reset, now), style: caption),
                        if (!reset.usable && reset.unavailable.isNotEmpty)
                          Text(
                            reset.unavailable,
                            style: TextStyles.caption.copyWith(
                              color: palette.textFaint,
                            ),
                          ),
                      ],
                    ),
                  ),
                  const SizedBox(width: Space.m),
                  Button(
                    label: 'Use…',
                    onPressed: () => unawaited(
                      useResetDialog(context, meter: meter, reset: reset),
                    ),
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// Asks before using [reset] on [meter]'s account, uses it, and says how
/// it went.
Future<void> useResetDialog(
  BuildContext context, {
  required UsageMeter meter,
  required UsageReset reset,
}) {
  final backend = AppScope.of(context);
  return showModal(
    context,
    (context, close) => UseResetDialog(
      backend: backend,
      meter: meter,
      reset: reset,
      close: close,
    ),
  );
}

/// Opens a page the backend points to; tests replace it.
@visibleForTesting
Future<bool> Function(Uri url) openResetPage = launchUrl;

class UseResetDialog extends StatefulWidget {
  const UseResetDialog({
    super.key,
    required this.backend,
    required this.meter,
    required this.reset,
    required this.close,
  });

  final Backend backend;
  final UsageMeter meter;
  final UsageReset reset;
  final VoidCallback close;

  @override
  State<UseResetDialog> createState() => _UseResetDialogState();
}

class _UseResetDialogState extends State<UseResetDialog> {
  var _using = false;
  String? _error;
  UsageResetAnswer? _answer;

  Future<void> _use() async {
    setState(() {
      _using = true;
      _error = null;
    });
    try {
      final answer = await widget.backend.useUsageReset(
        UseUsageReset(
          providerId: widget.meter.provider.id,
          accountId: widget.meter.account?.id ?? '',
          resetId: widget.reset.id,
        ),
      );
      if (mounted) setState(() => _answer = answer);
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _using = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final answer = _answer;
    if (answer != null) return _answered(context, answer);
    final palette = Palette.of(context);
    final reset = widget.reset;
    final meter = widget.meter;
    final caption = TextStyles.caption.copyWith(color: palette.textMuted);
    final who = meter.who.isEmpty
        ? meter.provider.name
        : '${meter.provider.name}, ${meter.who}';
    return FormFrame(
      title: 'Use a ${inSentence(reset.label)}?',
      body: reset.detail.isEmpty ? null : reset.detail,
      fields: [
        Text(who, style: caption),
        Text(resetCount(reset, DateTime.now()), style: caption),
        if (!reset.usable && reset.unavailable.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(top: Space.s),
            child: Text(
              '${reset.unavailable} You can try anyway.',
              style: TextStyles.caption.copyWith(color: palette.text),
            ),
          ),
        const SizedBox(height: Space.m),
      ],
      error: _error,
      submitLabel: _using ? 'Resetting…' : 'Use reset',
      onSubmit: _using ? null : _use,
      onCancel: widget.close,
    );
  }

  /// How it went, as the backend says, with the provider's own page when
  /// Claktory couldn't do it.
  Widget _answered(BuildContext context, UsageResetAnswer answer) {
    final palette = Palette.of(context);
    final page = Uri.tryParse(answer.fallbackUrl);
    final hasPage = answer.fallbackUrl.isNotEmpty && page != null;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          widget.reset.label,
          style: TextStyles.emphasis.copyWith(color: palette.text),
        ),
        const SizedBox(height: Space.s),
        Text(
          answer.message,
          style: TextStyles.body.copyWith(
            color: answer.reset ? palette.text : palette.textMuted,
          ),
        ),
        const SizedBox(height: Space.xl),
        Row(
          children: [
            const Spacer(),
            if (hasPage) ...[
              Button(
                label: 'Open ${pageName(widget.meter.provider.id, page)}',
                onPressed: () {
                  unawaited(openResetPage(page));
                  widget.close();
                },
              ),
              const SizedBox(width: Space.s),
            ],
            Button(label: 'Done', onPressed: widget.close, primary: true),
          ],
        ),
      ],
    );
  }
}

/// [label] in the middle of a sentence: "Weekly reset" is "weekly reset",
/// and a name ("Fable reset") stays as it is.
String inSentence(String label) {
  const common = {'Weekly', 'Full', 'Daily', 'Monthly', 'Hourly'};
  final space = label.indexOf(' ');
  final first = space < 0 ? label : label.substring(0, space);
  if (!common.contains(first)) return label;
  return label[0].toLowerCase() + label.substring(1);
}

/// The provider's site as people say it ("claude.ai", "chatgpt.com"),
/// whatever short link [page] is; else [page]'s own.
String pageName(String providerId, Uri page) => switch (providerId) {
  'anthropic' => 'claude.ai',
  'openai' || 'openai-codex' => 'chatgpt.com',
  _ => page.host.startsWith('www.') ? page.host.substring(4) : page.host,
};

/// A usage circle with, when [shown], a small dot at its top right: a limit
/// is nearly used and a reset can refill it (hover for the resets).
class ResetCue extends StatelessWidget {
  const ResetCue({super.key, required this.shown, required this.child});

  final bool shown;
  final Widget child;

  @override
  Widget build(BuildContext context) => Stack(
    clipBehavior: Clip.none,
    children: [
      Positioned.fill(child: child),
      if (shown)
        Positioned(
          top: 1,
          right: 1,
          child: Semantics(
            label: 'Close to a limit. A usage reset can refill it.',
            child: Dot(color: Palette.of(context).signal),
          ),
        ),
    ],
  );
}
