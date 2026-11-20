import 'dart:convert';

import 'package:crypto/crypto.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'devices.dart' show thisDeviceKind;
import 'identity.dart';
import 'markdown.dart' show CodeBlock;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// The four digits a new device shows while it waits to be allowed, and
/// the Claktory it asks shows too (PendingDevice.number): worked out on
/// each side from both endpoint ids ([backendId], [deviceId], lowercase
/// hex) and the pairing code's [token], so a device that copied the code,
/// with a key of its own, shows other digits.
String pairingNumber({
  required String backendId,
  required String deviceId,
  required String token,
}) {
  final tokenHash = sha256.convert(utf8.encode(token)).toString();
  final digest = sha256
      .convert(
        utf8.encode('claktory pairing\n$backendId\n$deviceId\n$tokenHash'),
      )
      .bytes;
  final value = digest[0] << 24 | digest[1] << 16 | digest[2] << 8 | digest[3];
  return (value % 10000).toString().padLeft(4, '0');
}

/// This device waits for the person to allow it on the Claktory it used a
/// code for: what the backend said ([pending]) and the digits to compare.
class PairingWait {
  const PairingWait(this.pending, this.number);

  final PairingPending pending;
  final String number;
}

/// The digits both sides show, large and spaced, to compare at a glance.
class PairingDigits extends StatelessWidget {
  const PairingDigits(this.number, {super.key});

  final String number;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      label: 'Number ${number.split('').join(' ')}',
      excludeSemantics: true,
      child: Text(
        number,
        textAlign: TextAlign.center,
        style: TextStyle(
          fontFamily: TextStyles.mono,
          fontSize: 28,
          height: 1.2,
          fontWeight: FontWeight.w500,
          letterSpacing: 10,
          color: palette.text,
          fontFeatures: const [FontFeature.tabularFigures()],
        ),
      ),
    );
  }
}

/// The command that allows this device on a Claktory without a screen
/// (a server), from its terminal: only if it shows the same number.
String allowCommand(PairingWait wait) =>
    'factoryd allow ${wait.pending.deviceId} --number ${wait.number}';

/// [command] in a code block, with Copy.
class AllowCommand extends StatefulWidget {
  const AllowCommand(this.command, {super.key});

  final String command;

  @override
  State<AllowCommand> createState() => _AllowCommandState();
}

class _AllowCommandState extends State<AllowCommand> {
  var _copied = false;

  @override
  Widget build(BuildContext context) => Row(
    children: [
      Expanded(child: CodeBlock(widget.command)),
      const SizedBox(width: Space.s),
      Button(
        label: _copied ? 'Copied' : 'Copy',
        onPressed: () {
          Clipboard.setData(ClipboardData(text: widget.command));
          setState(() => _copied = true);
        },
      ),
    ],
  );
}

/// This device used a pairing code and waits for the person to allow it
/// on that Claktory: the number to compare with the one it shows, and
/// Cancel; turned away, why, and Use a new code. [onCancel] gives up on it.
class PairingWaitView extends StatelessWidget {
  const PairingWaitView({super.key, required this.onCancel});

  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: Listenable.merge([backend.pairing, backend.refused]),
      builder: (context, _) {
        final wait = backend.pairing.value;
        final refused = backend.refused.value;
        final machine = wait?.pending.machine.isNotEmpty ?? false
            ? wait!.pending.machine
            : backend.target?.label ?? 'your Mac';
        return Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(Space.xl),
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 420),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(
                    refused != null
                        ? "Couldn't connect to $machine"
                        : 'Waiting for $machine',
                    style: TextStyles.title.copyWith(color: palette.text),
                  ),
                  const SizedBox(height: Space.s),
                  if (refused != null)
                    Text(
                      refused,
                      style: TextStyles.body.copyWith(color: palette.danger),
                    )
                  else ...[
                    Text(
                      'Allow this $thisDeviceKind on $machine. Its screen '
                      'shows the same number:',
                      style: TextStyles.body.copyWith(color: palette.textMuted),
                    ),
                    if (wait != null && wait.number.isNotEmpty) ...[
                      const SizedBox(height: Space.xl),
                      PairingDigits(wait.number),
                      // A server has no screen: it's allowed from its
                      // terminal, with the number checked there.
                      if (wait.pending.deviceId.isNotEmpty) ...[
                        const SizedBox(height: Space.xl),
                        Text(
                          'No screen on $machine? Run this there:',
                          style: TextStyles.body.copyWith(
                            color: palette.textMuted,
                          ),
                        ),
                        const SizedBox(height: Space.s),
                        AllowCommand(allowCommand(wait)),
                      ],
                    ],
                  ],
                  const SizedBox(height: Space.xl),
                  Align(
                    alignment: Alignment.centerRight,
                    child: Button(
                      label: refused != null ? 'Use a new code' : 'Cancel',
                      primary: refused != null,
                      onPressed: onCancel,
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}
