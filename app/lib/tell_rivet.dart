import 'package:flutter/widgets.dart';

import 'backend.dart' show RequestFailure;
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart' show Agent;
import 'scope.dart';
import 'usage_wait.dart';

/// What Rivet is asked when the person hands it an error: the error in
/// full, after [about] (where it showed) when there's one.
String errorRequest(String error, {String about = ''}) {
  final where = about.isEmpty ? '' : '$about\n';
  return 'Claktory shows this error. Please fix it:\n\n$where$error';
}

/// Hands an error Factory shows to Rivet, as the sidebar's "+" hands it a
/// request: Rivet's conversation opens with the error typed in its message
/// box, ready to send. Nothing shows when there's no Rivet, or it has no
/// model to run. A quiet button, or as [link] a caption link for an error
/// that's itself a line of caption.
class TellRivet extends StatelessWidget {
  const TellRivet({
    super.key,
    required this.error,
    this.about = '',
    this.onTold,
    this.link = false,
  });

  final String error;

  /// Where the error showed, said before it: "The settings file has an
  /// error.", "Builder's last run failed."
  final String about;

  /// After it's handed over, such as closing the dialog it showed in.
  final VoidCallback? onTold;
  final bool link;

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        final rivet = rivetIn(store.workspace);
        if (rivet == null || !rivet.modelAvailable || error.isEmpty) {
          return const SizedBox.shrink();
        }
        // Asleep on a usage limit, Rivet wouldn't read it until the reset:
        // its wait instead, and the way to carry on now.
        if (rivet.usageWait.isNotEmpty) {
          return _RivetWaits(rivet: rivet, link: link);
        }
        final label = 'Ask ${rivet.name} to fix';
        void tell() {
          if (askRivet(
            context,
            errorRequest(error, about: about),
            hint: 'Send it to ${rivet.name} to fix',
          )) {
            onTold?.call();
          }
        }

        if (!link) return Button(label: label, onPressed: tell);
        final palette = Palette.of(context);
        return Semantics(
          button: true,
          label: label,
          onTap: tell,
          excludeSemantics: true,
          child: Pressable(
            onTap: tell,
            builder: (context, hovered) => Text(
              label,
              style: TextStyles.caption.copyWith(
                color: hovered ? palette.text : palette.textMuted,
                fontWeight: FontWeight.w500,
                decoration: hovered ? TextDecoration.underline : null,
                decorationColor: palette.text,
              ),
            ),
          ),
        );
      },
    );
  }
}

/// In [TellRivet]'s place while Rivet waits out a usage limit: "Rivet is
/// waiting for your Claude usage to reset at Fri 18:00", and Continue on
/// another provider when one can run it, or Sign in… when none is
/// signed in. As [link], caption links after the words; else buttons.
class _RivetWaits extends StatefulWidget {
  const _RivetWaits({required this.rivet, required this.link});

  final Agent rivet;
  final bool link;

  @override
  State<_RivetWaits> createState() => _RivetWaitsState();
}

class _RivetWaitsState extends State<_RivetWaits> {
  var _busy = false;
  String? _error;

  Future<void> _continue() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      // Rivet alone: whether others go too is asked on their wait lines.
      await AppScope.of(context).continueOnAlternative(widget.rivet.id);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final rivet = widget.rivet;
    final line = usageWaitLine(rivet);
    final words = line.startsWith('Waiting ')
        ? '${rivet.name} is w${line.substring(1)}'
        : '${rivet.name}: $line';
    final signingIn = line.startsWith('Waiting for you to sign in to');
    // A switch changes the settings file, so not while it's broken (the
    // settings-file error dialog shows this): the wait alone then.
    final broken = AppScope.of(context)
        .workspace
        .workspace
        .configError
        .isNotEmpty;
    final alternative = rivet.hasUsageWaitAlternative();
    final actions = <(String, VoidCallback?)>[
      if (alternative && !broken)
        (
          'Continue on ${rivet.usageWaitAlternative.providerName}',
          _busy ? null : _continue,
        )
      else if (!alternative && (rivet.usageWaitNoOtherProvider || signingIn))
        ('Sign in…', () => AppScope.openProviders(context)),
    ];
    final caption = TextStyles.caption;
    final said = Text(words, style: caption.copyWith(color: palette.textMuted));
    final error = _error == null
        ? null
        : Text(_error!, style: caption.copyWith(color: palette.danger));
    if (widget.link) {
      return Wrap(
        spacing: Space.s,
        crossAxisAlignment: WrapCrossAlignment.end,
        children: [
          said,
          for (final (label, onTap) in actions)
            Semantics(
              button: true,
              label: label,
              onTap: onTap,
              excludeSemantics: true,
              child: Pressable(
                onTap: onTap,
                builder: (context, hovered) => Text(
                  label,
                  style: caption.copyWith(
                    color: hovered ? palette.text : palette.textMuted,
                    fontWeight: FontWeight.w500,
                    decoration: hovered ? TextDecoration.underline : null,
                    decorationColor: palette.text,
                  ),
                ),
              ),
            ),
          ?error,
        ],
      );
    }
    // Among other buttons in a row (a dialog's, a banner's): its button,
    // with the words under it at about its width, so the row keeps its
    // length.
    return ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 200),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          for (final (label, onPressed) in actions) ...[
            Button(label: label, onPressed: onPressed),
            const SizedBox(height: Space.xs),
          ],
          Text(
            words,
            textAlign: TextAlign.end,
            style: caption.copyWith(color: palette.textMuted),
          ),
          if (_error case final failure?)
            Text(
              failure,
              textAlign: TextAlign.end,
              style: caption.copyWith(color: palette.danger),
            ),
        ],
      ),
    );
  }
}

/// An error line with [TellRivet] after it, wrapping when it's long.
class ErrorLine extends StatelessWidget {
  const ErrorLine(this.text, {super.key, this.about = '', this.style});

  final String text;
  final String about;

  /// The error's style; caption `danger` unless given.
  final TextStyle? style;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Wrap(
      spacing: Space.s,
      crossAxisAlignment: WrapCrossAlignment.end,
      children: [
        Text(
          text,
          style: style ?? TextStyles.caption.copyWith(color: palette.danger),
        ),
        TellRivet(error: text, about: about, link: true),
      ],
    );
  }
}
