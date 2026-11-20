import 'package:flutter/widgets.dart';

import 'connections.dart';
import 'controls.dart';
import 'identity.dart';
import 'name_step.dart' show StepCard;
import 'rail.dart' show ClaktoriesScope;
import 'scope.dart';

/// Whether the window asks to connect to a Claktory: this Mac has none of
/// its own (it uses one on another computer) and shows none yet. A phone
/// asks on its own screen (PairScreen).
bool asksToConnect(BuildContext context) {
  final claktories = ClaktoriesScope.of(context);
  if (claktories == null || claktories.localBackend) return false;
  return AppScope.scope(context).connections.current == null &&
      !isPhoneLayout(context);
}

/// A Mac that uses a Claktory on another computer, with none yet: its
/// code, pasted, connects to it (then that Claktory allows this Mac). Or
/// Claktory is set up on this Mac after all.
class ConnectStep extends StatefulWidget {
  const ConnectStep({super.key});

  @override
  State<ConnectStep> createState() => _ConnectStepState();
}

class _ConnectStepState extends State<ConnectStep> {
  final _code = TextEditingController();

  @override
  void dispose() {
    _code.dispose();
    super.dispose();
  }

  void _connect(RemoteFactory target) =>
      AppScope.switchFactory(context, target);

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final claktories = ClaktoriesScope.of(context);
    return StepCard(
      child: ListenableBuilder(
        listenable: _code,
        builder: (context, _) {
          final pasted = _code.text.trim();
          final target = parsePairingCode(pasted);
          final usable = target != null && target.tunnelled;
          final error = pasted.isEmpty
              ? null
              : target == null
              ? "That isn't a Claktory code."
              : !target.tunnelled
              ? 'This code is from an older Claktory. Update it, then make '
                    'a new code.'
              : null;
          return Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                'Connect to your Claktory',
                style: TextStyles.emphasis.copyWith(color: palette.text),
              ),
              const SizedBox(height: Space.s),
              Text(
                'On the computer Claktory runs on, open Settings → Devices → '
                'Add a device. On a server, run `factoryd pair`. Then paste '
                'the code here.',
                style: TextStyles.body.copyWith(color: palette.textMuted),
              ),
              const SizedBox(height: Space.xl),
              TextField(
                controller: _code,
                placeholder: 'Paste code',
                autofocus: true,
                onSubmitted: (_) {
                  if (usable) _connect(target);
                },
              ),
              if (error != null) ...[
                const SizedBox(height: Space.s),
                Text(
                  error,
                  style: TextStyles.caption.copyWith(color: palette.danger),
                ),
              ],
              const SizedBox(height: Space.xl),
              Wrap(
                alignment: WrapAlignment.end,
                spacing: Space.s,
                runSpacing: Space.s,
                children: [
                  if (claktories != null && claktories.canChooseLocal)
                    Button(
                      label: 'Set up Claktory on this Mac',
                      onPressed: () => claktories.setLocalBackend(true),
                    ),
                  Button(
                    label: 'Connect',
                    primary: true,
                    onPressed: usable ? () => _connect(target) : null,
                  ),
                ],
              ),
            ],
          );
        },
      ),
    );
  }
}
