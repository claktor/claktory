import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'claktory_identity.dart' show claktoryName;
import 'controls.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'tell_rivet.dart' show ErrorLine;

/// Needs you's question while Workspace.keep_awake.ask (#352 P1, Matt's
/// O1): whether Claktory keeps its Mac awake while agents work, only on
/// power and with the lid open. Trish's words.
const keepAwakeTitle = "Keep your agents working while you're away?";

/// The Mac Claktory runs on: "this Mac" on that Mac itself, else its
/// Claktory's name ("Studio"), since a phone isn't "this Mac".
String keepAwakeMac(Backend backend) =>
    backend.localBackend && backend.target == null
    ? 'this Mac'
    : claktoryName(backend);

/// What a yes does: the line under the question's title, and how a yes
/// ended.
String keepAwakeSummary(Backend backend) =>
    'Agents keep working while ${keepAwakeMac(backend)} is plugged in with '
    'its lid open.';

/// The rest, in the opened card above the answers.
const keepAwakeBody = 'You can change this in Settings → General.';

/// Settings' switch, its note on a laptop, and while it holds.
String keepAwakeLabel(Backend backend) =>
    'Keep ${keepAwakeMac(backend)} awake while agents work';
const keepAwakeNote = "Only while it's plugged in with its lid open.";
String keepAwakeHolding(Backend backend) =>
    'Keeping ${keepAwakeMac(backend)} awake now';

/// On instead while agents work, but on battery: nothing holds it.
String keepAwakeOnBattery(Backend backend) =>
    'On battery, so ${keepAwakeMac(backend)} can sleep. Plug it in to keep '
    'agents working.';

/// How it ended, once answered: what the Mac does now.
String keepAwakeAnswered(Backend backend, KeepAwake keep) {
  if (keep.on) return keepAwakeSummary(backend);
  final mac = keepAwakeMac(backend);
  // A Claktory's name as it was typed ("studio"), never capitalized.
  return '${mac == 'this Mac' ? 'This Mac' : mac} sleeps as usual.';
}

/// The answers: Keep it awake is SetKeepAwake on, Let it sleep off (as a
/// dismissal would be).
class KeepAwakeCard extends StatefulWidget {
  const KeepAwakeCard({super.key});

  @override
  State<KeepAwakeCard> createState() => _KeepAwakeCardState();
}

class _KeepAwakeCardState extends State<KeepAwakeCard> {
  var _busy = false;
  String? _failure;

  Future<void> _answer(bool on) async {
    final backend = AppScope.of(context);
    setState(() {
      _busy = true;
      _failure = null;
    });
    try {
      await backend.setKeepAwake(on);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _failure = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          keepAwakeBody,
          style: TypeScale.of(context).caption
              .copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.s),
        Wrap(
          spacing: Space.s,
          runSpacing: Space.s,
          children: [
            Button(
              label: 'Keep it awake',
              primary: true,
              onPressed: _busy ? null : () => _answer(true),
            ),
            Button(
              label: 'Let it sleep',
              onPressed: _busy ? null : () => _answer(false),
            ),
          ],
        ),
        if (_failure case final failure?) ...[
          const SizedBox(height: Space.s),
          ErrorLine(
            "Couldn't keep ${keepAwakeMac(AppScope.of(context))} awake: "
            '$failure',
            about: 'Keeping the Mac awake:',
          ),
        ],
      ],
    );
  }
}
