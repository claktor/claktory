import 'dart:async';

import 'package:flutter/widgets.dart';

import 'connections.dart';
import 'controls.dart';
import 'devices.dart' show thisDeviceKind;
import 'identity.dart';
import 'links.dart' show confirmPairing;
import 'mobile/local_network.dart' show connectNewClaktory;
import 'name_step.dart' show StepCard;
import 'pasted_code.dart';
import 'rail.dart' show ClaktoriesScope;
import 'scope.dart';
import 'your_claktories.dart' show YourClaktories;

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

  /// Connecting through the account (sign in, pick one of "Your
  /// Claktories") rather than with a code: at first when this Mac is
  /// signed in already.
  bool? _withAccount;

  @override
  void dispose() {
    _code.dispose();
    super.dispose();
  }

  /// One of the person's own, from their account: connects, the first
  /// Claktory on an iPad asking for Local Network first, as every way in
  /// does (devices-signin.md §8.8).
  void _connect(RemoteFactory target) =>
      unawaited(connectNewClaktory(context, target));

  /// A pasted code connects only once the person says so, as every way a
  /// code comes in does (confirmPairing, Fugo).
  Future<void> _connectPasted(RemoteFactory target) async {
    // One question at a time: a second click while it asks does nothing
    // (Abbacchio).
    if (_asking) return;
    _asking = true;
    final bool yes;
    try {
      yes = await confirmPairing(context, target);
    } finally {
      _asking = false;
    }
    if (!yes || !mounted) return;
    _connect(target);
  }

  /// Whether [confirmPairing] is asking about a pasted code.
  var _asking = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final claktories = ClaktoriesScope.of(context);
    final withAccount = _withAccount ??= AppScope.scope(context)
        .connections
        .account
        .signedIn;
    if (withAccount) {
      return StepCard(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              'Connect to your Claktory',
              style: TextStyles.emphasis.copyWith(color: palette.text),
            ),
            const SizedBox(height: Space.xl),
            YourClaktories(onConnect: _connect),
            const SizedBox(height: Space.l),
            Align(
              alignment: Alignment.centerLeft,
              child: Button(
                label: 'Use a code instead',
                onPressed: () => setState(() => _withAccount = false),
              ),
            ),
          ],
        ),
      );
    }
    return StepCard(
      child: ListenableBuilder(
        listenable: _code,
        builder: (context, _) {
          final pasted = _code.text.trim();
          final target = pastedPairingCode(pasted);
          final usable = target != null && target.tunnelled;
          final error = pasted.isEmpty
              ? null
              : target == null
              ? otherCodeProblem(
                      pasted,
                      device: thisDeviceKind,
                      where: whereItRuns,
                    ) ??
                    "That isn't a Claktory code."
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
                  if (usable) unawaited(_connectPasted(target));
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
                  Button(
                    label: 'Sign in with your email instead',
                    onPressed: () => setState(() => _withAccount = true),
                  ),
                  if (claktories != null && claktories.canChooseLocal)
                    Button(
                      label: 'Set up Claktory on this Mac',
                      onPressed: () => claktories.setLocalBackend(true),
                    ),
                  Button(
                    label: 'Connect',
                    primary: true,
                    onPressed: usable
                        ? () => unawaited(_connectPasted(target))
                        : null,
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
