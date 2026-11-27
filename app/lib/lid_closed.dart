// Keeping a laptop's agents working with its lid closed
// (coordination/security/lid-closed.md): Settings → General → Sleep's
// switch for it, its own question before it's turned on, and where it
// stands (Workspace.keep_awake.lid_closed). The backend itself keeps the
// Mac awake with its lid closed while agents work and it's plugged in, and
// lets go when it stops; nothing is installed. Turning it on works only in
// the Mac app on that Mac; any other device shows where it stands and can
// turn it off. A Mac that can't sleep although Claktory isn't keeping it
// awake (STUCK) says so everywhere, with what fixes it.
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'keep_awake_card.dart' show keepAwakeMac;
import 'mobile/metrics.dart' show TypeScale;
import 'pairing.dart' show AllowCommand;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// This app is the Mac app on the Mac [backend]'s Claktory runs on: the
/// only place it's turned on.
bool lidClosedHere(Backend backend) =>
    defaultTargetPlatform == TargetPlatform.macOS &&
    backend.localBackend &&
    backend.target == null;

// The words.

const lidClosedLabel = 'Keep agents working with the lid closed';
const lidClosedNote = "Only while it's plugged in and agents are working.";
const lidClosedFromMac = 'Turn it on from the Mac.';

const lidClosedQuestionTitle = 'Keep agents working with the lid closed?';
String lidClosedQuestionBody(Backend backend) =>
    '${_capital(keepAwakeMac(backend))} stays awake while agents work and '
    "it's plugged in, even with its lid closed.";
const lidClosedQuestionBag =
    "Don't put it in a bag while it's on: a Mac that stays awake with its "
    'lid closed gets hot. Claktory lets it sleep as soon as it\'s unplugged, '
    'and when it gets too hot.';
const lidClosedQuestionOff = 'You can turn it off here at any time.';

/// What the setting says under its switch, by where it stands; null for
/// nothing. The states a helper-era backend sent (NEEDS_APPROVAL, the
/// battery pauses, MISSING, OUTDATED) read as OFF.
({String text, bool holding, bool failed})? lidClosedStatus(
  Backend backend,
  KeepAwake keep,
) {
  final mac = keepAwakeMac(backend);
  final status = switch (keep.lidClosed) {
    LidClosedState.LID_CLOSED_ON =>
      keep.lidClosedHolding
          ? 'Keeping $mac awake now, even with its lid closed'
          : 'On: agents keep working while $mac is plugged in, even with its '
                'lid closed.',
    LidClosedState.LID_CLOSED_PAUSED_HEAT =>
      'Paused: $mac is too hot. It starts again 10 minutes after it cools '
          'down.',
    LidClosedState.LID_CLOSED_FAILED =>
      keep.lidClosedError.isEmpty
          ? "Couldn't keep $mac awake with its lid closed."
          : "Couldn't keep $mac awake with its lid closed: "
                '${keep.lidClosedError}',
    _ => null,
  };
  if (status == null) return null;
  return (
    text: status,
    holding:
        keep.lidClosed == LidClosedState.LID_CLOSED_ON && keep.lidClosedHolding,
    failed: keep.lidClosed == LidClosedState.LID_CLOSED_FAILED,
  );
}

/// The person's setting, STUCK included: a backend
/// from before lid_closed_turned_on says it by its state, the retired ones
/// counting as off.
bool lidClosedTurnedOn(KeepAwake keep) =>
    keep.lidClosedTurnedOn ||
    switch (keep.lidClosed) {
      LidClosedState.LID_CLOSED_ON ||
      LidClosedState.LID_CLOSED_PAUSED_HEAT ||
      LidClosedState.LID_CLOSED_FAILED => true,
      _ => false,
    };

/// STUCK's heading: the Mac can't sleep with its lid closed although
/// Claktory isn't keeping it awake.
String lidClosedStuck(Backend backend) =>
    "${_capital(keepAwakeMac(backend))} can't sleep with its lid closed right "
    'now.';

String _capital(String words) =>
    words.isEmpty ? words : words[0].toUpperCase() + words.substring(1);

/// [fix] (KeepAwake.lid_closed_fix) as it reads: its words, and each
/// command in backticks apart, to copy.
List<({String text, bool command})> lidClosedFixParts(String fix) {
  final parts = <({String text, bool command})>[];
  final pieces = fix.split('`');
  // An odd backtick count leaves the last piece unclosed: it's words.
  for (var i = 0; i < pieces.length; i++) {
    final command = i.isOdd && i < pieces.length - 1;
    final text = command ? pieces[i].trim() : pieces[i];
    if (text.trim().isEmpty) continue;
    parts.add((text: text, command: command));
  }
  return parts;
}

/// Asks before turning it on (its own question), and turns it on if the
/// person says so.
Future<void> askLidClosed(BuildContext context, Backend backend) => showModal(
  context,
  (context, close) => _LidClosedQuestion(close: close, backend: backend),
);

/// STUCK, wherever Sleep shows, the switch with it or not: what's wrong,
/// in the danger color, and what fixes it, its commands with Copy.
class LidClosedStuck extends StatelessWidget {
  const LidClosedStuck({super.key, required this.keep, this.heading = true});

  final KeepAwake keep;

  /// Says what's wrong first; Needs you's item says it as its title.
  final bool heading;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final parts = lidClosedFixParts(keep.lidClosedFix);
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (heading)
          Text(
            lidClosedStuck(backend),
            style: type.body.copyWith(color: palette.danger),
          ),
        if (parts.isNotEmpty) ...[
          if (heading) const SizedBox(height: Space.xs),
          // One sentence, its commands inline, so no words are left alone
          // after a command's box; the boxes to copy
          // them from follow it.
          Text.rich(
            TextSpan(
              children: [
                for (final (i, part) in parts.indexed)
                  TextSpan(
                    text: i == 0 ? part.text.trimLeft() : part.text,
                    style: part.command
                        ? type.code.copyWith(color: palette.text)
                        : null,
                  ),
              ],
            ),
            style: type.caption.copyWith(color: palette.textMuted),
          ),
        ],
        for (final part in parts)
          if (part.command) ...[
            const SizedBox(height: Space.xs),
            AllowCommand(part.text),
          ],
      ],
    );
  }
}

/// Its own question, before it's turned on (never bundled with keeping
/// the Mac awake): what it does, the heat and the bag.
class _LidClosedQuestion extends StatefulWidget {
  const _LidClosedQuestion({required this.close, required this.backend});

  final VoidCallback close;
  final Backend backend;

  @override
  State<_LidClosedQuestion> createState() => _LidClosedQuestionState();
}

class _LidClosedQuestionState extends State<_LidClosedQuestion> {
  var _busy = false;
  String? _error;

  Future<void> _turnOn() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.backend.setLidClosed(on: true);
      widget.close();
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
    Widget line(String text) => Padding(
      padding: const EdgeInsets.only(bottom: Space.m),
      child: Text(text, style: type.body.copyWith(color: palette.text)),
    );
    return FormFrame(
      title: lidClosedQuestionTitle,
      body: lidClosedQuestionBody(widget.backend),
      fields: [line(lidClosedQuestionBag), line(lidClosedQuestionOff)],
      error: _error,
      submitLabel: 'Turn on',
      cancelLabel: 'Not now',
      onSubmit: _busy ? null : () => unawaited(_turnOn()),
      onCancel: widget.close,
    );
  }
}
