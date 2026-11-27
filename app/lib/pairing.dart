import 'dart:convert';
import 'dart:math' as math;

import 'package:crypto/crypto.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'approve_elsewhere.dart';
import 'backend.dart' show Backend, RequestFailure;
import 'claktory_problem.dart'
    show ClaktoryProblemPage, ProblemAction, ProblemText;
import 'controls.dart';
import 'devices.dart' show thisDeviceKind;
import 'identity.dart';
import 'markdown.dart' show CodeBlock;
import 'mobile/input.dart' show InputKind;
import 'mobile/metrics.dart' show TypeScale;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'visible_codes.dart' show nameInText;

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
    // A waiting device's digits never show on Windows (approve_elsewhere.dart).
    if (approvesElsewhere) return const ApproveElsewhere();
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
      Expanded(
        child: isPhoneLayout(context)
            ? _WrappedCommand(widget.command)
            : CodeBlock(widget.command),
      ),
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

/// A command on a phone: CodeBlock's box, wrapped onto more lines (broken
/// anywhere) rather than scrolled sideways out of sight, so what Copy
/// copies can be read (Lucy 49057).
class _WrappedCommand extends StatelessWidget {
  const _WrappedCommand(this.command);

  final String command;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      padding: const EdgeInsets.symmetric(
        horizontal: Space.m,
        vertical: Space.s + 2,
      ),
      child: Semantics(
        label: command,
        excludeSemantics: true,
        // A zero-width space after every character lets it break
        // anywhere; Copy copies [command] itself.
        child: Text(
          command.characters.join('\u200B'),
          style: TypeScale.of(context).code.copyWith(color: palette.text),
        ),
      ),
    );
  }
}

/// This device used a pairing code and waits for the person to allow it
/// on that Claktory: the number to compare with the one it shows, and
/// Cancel; turned away, why, and Use a new code. [onCancel] gives up on it.
class PairingWaitView extends StatefulWidget {
  const PairingWaitView({super.key, required this.onCancel});

  final VoidCallback onCancel;

  @override
  State<PairingWaitView> createState() => _PairingWaitViewState();
}

class _PairingWaitViewState extends State<PairingWaitView> {
  /// Whether the person asked for the command that allows this device on
  /// a Claktory without a screen. Folded away at first: most Claktories
  /// have their app open, and the number is what they compare.
  var _commandShown = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: Listenable.merge([backend.pairing, backend.refused]),
      builder: (context, _) {
        final wait = backend.pairing.value;
        final refused = backend.refused.value;
        // The code's name, as the strip and the confirm sheet say it
        // (Zucchero's row 720); without one, the backend's hostname
        // rather than an address.
        final target = backend.target;
        // Whoever chose it (the account service's list, say), it reads
        // as a name: nothing in it turns the sentence around (row 1784).
        final machine = target != null && target.name.isNotEmpty
            ? nameInText(target.name)
            : wait?.pending.machine.isNotEmpty ?? false
            ? nameInText(wait!.pending.machine)
            : target != null
            ? nameInText(target.label)
            : 'your Mac';
        final muted = TextStyles.body.copyWith(color: palette.textMuted);
        if (refused != null) {
          return ClaktoryProblemPage(
            title: "Couldn't connect to $machine",
            body: [
              Text(
                refused,
                style: TextStyles.body.copyWith(color: palette.danger),
              ),
            ],
            primary: ProblemAction('Use a new code', widget.onCancel),
          );
        }
        return ClaktoryProblemPage(
          title: 'Waiting for $machine',
          body: [
            // Since #546 the allowing device is never sent the digits:
            // the person types what this screen shows there (Trish).
            ProblemText(
              'Allow this $thisDeviceKind on $machine, or on another device '
              'that uses $machine, by typing the number this '
              '$thisDeviceKind shows there:',
            ),
            if (wait != null && wait.number.isNotEmpty) ...[
              const SizedBox(height: Space.m),
              PairingDigits(wait.number),
              // A server has no screen: it's allowed from its terminal,
              // with the number checked there. Folded away until asked
              // for.
              if (wait.pending.deviceId.isNotEmpty) ...[
                const SizedBox(height: Space.m),
                if (_commandShown) ...[
                  Text(
                    'No screen on $machine? Run this in a terminal there:',
                    style: muted,
                  ),
                  AllowCommand(allowCommand(wait)),
                  // Without a terminal (ssh without -t), a Claktory that
                  // has devices refuses it (Matt's case, Fugo's ruling
                  // 35297).
                  Text(
                    'Over SSH, connect with ssh -t so it runs in a terminal.',
                    style: muted,
                  ),
                ] else
                  Align(
                    alignment: AlignmentDirectional.centerStart,
                    child: Button(
                      label: 'No screen on $machine?',
                      onPressed: () => setState(() => _commandShown = true),
                    ),
                  ),
              ],
            ],
          ],
          // Taken back there too, so it leaves that Claktory's list at
          // once rather than when it runs out (#603).
          quiet: [
            ProblemAction('Cancel', () {
              backend.withdrawPairing();
              widget.onCancel();
            }),
          ],
        );
      },
    );
  }
}

/// Whether [number] is a waiting device's number in full: four digits.
bool isDeviceNumber(String number) => RegExp(r'^\d{4}$').hasMatch(number);

/// How a waiting device is named (Fugo's rule 3): the name it gave itself,
/// quoted as its own words, never as Claktory's ("A device that calls
/// itself “Ana's iPhone”").
String deviceCallingItself(PendingDevice device) => device.name.isEmpty
    ? 'A device'
    : 'A device that calls itself “${nameInText(device.name)}”';

/// The four digits a waiting device shows, typed here to allow it (#352
/// P4): the backend compares them with the device's own, so only someone
/// who can see the device lets it in. Digits only, at most four.
class DeviceNumberField extends StatelessWidget {
  const DeviceNumberField({
    super.key,
    required this.controller,
    this.onSubmitted,
    this.autofocus = false,
  });

  final TextEditingController controller;
  final ValueChanged<String>? onSubmitted;
  final bool autofocus;

  /// How wide the field is where it sits beside or under other things:
  /// room for all four digits and the caret at the text size it shows
  /// (Zucchero's row 265: at the largest, a fixed width hid one), and never
  /// narrower than [least].
  static double widthFor(BuildContext context, {double least = 96}) {
    final style = TypeScale.of(context).body;
    final digits = TextPainter(
      text: TextSpan(text: '00000', style: style),
      textDirection: TextDirection.ltr,
      textScaler: MediaQuery.textScalerOf(context),
      maxLines: 1,
    )..layout();
    // The field's own padding and border, a phone's being the wider.
    final width = digits.width + 2 * Space.m + 2;
    digits.dispose();
    return math.max(least, width.ceilToDouble());
  }

  @override
  Widget build(BuildContext context) => TextField(
    controller: controller,
    placeholder: '0000',
    kind: InputKind.number,
    autofocus: autofocus,
    semanticsLabel: 'Number on its screen',
    inputFormatters: [
      FilteringTextInputFormatter.digitsOnly,
      LengthLimitingTextInputFormatter(4),
    ],
    // A number pad has no Done key: scrolled into view, the field brings
    // Allow under it into sight too.
    scrollPadding: const EdgeInsets.fromLTRB(20, 20, 20, 160),
    onSubmitted: onSubmitted,
  );
}

/// Whether [a] and [b] are the same request to be allowed: the same
/// device asking the same time, with the same number.
bool sameDeviceRequest(PendingDevice? a, PendingDevice? b) =>
    a != null &&
    b != null &&
    a.deviceId == b.deviceId &&
    a.number == b.number &&
    a.requestedMs == b.requestedMs;

/// Allows [device] with the [number] typed, or turns it away ([allow]
/// false); null when that went through, else why not, in the backend's
/// words (a wrong number, with the tries left) or, once it no longer
/// waits, [stoppedWaiting] ("… stopped waiting.").
Future<String?> decideDevice(
  Backend backend,
  PendingDevice device, {
  required bool allow,
  String number = '',
  String? stoppedWaiting,
}) async {
  try {
    if (allow) {
      await backend.approveDevice(device.deviceId, number);
    } else {
      await backend.denyDevice(device.deviceId);
    }
    return null;
  } on RequestFailure catch (failure) {
    return failure.code == ErrorCode.ERROR_CODE_NOT_FOUND
        ? stoppedWaiting ?? '${deviceCallingItself(device)} stopped waiting.'
        : failure.message;
  }
}

/// A device waiting to be allowed on this Claktory, as Needs you shows it
/// (#352 P4): the name it gave itself, what to do if it isn't the
/// person's, the number its screen shows to type, and Allow or Don't
/// allow. [device] null once it no longer waits: how it ended here.
class AllowDeviceCard extends StatefulWidget {
  const AllowDeviceCard({super.key, required this.device});

  final PendingDevice? device;

  @override
  State<AllowDeviceCard> createState() => _AllowDeviceCardState();
}

class _AllowDeviceCardState extends State<AllowDeviceCard> {
  final _number = TextEditingController();
  var _deciding = false;
  String? _error;

  /// How it ended here: Allowed, or Not allowed.
  String? _ended;

  @override
  void initState() {
    super.initState();
    _number.addListener(() => setState(() {}));
  }

  @override
  void didUpdateWidget(AllowDeviceCard old) {
    super.didUpdateWidget(old);
    // Another request, even from the same device (its id kept when it
    // pairs again): it starts afresh, with nothing typed for the last one
    // (Zucchero's row 264).
    final device = widget.device;
    if (device != null && !sameDeviceRequest(old.device, device)) {
      _number.clear();
      _error = null;
      _ended = null;
    }
  }

  @override
  void dispose() {
    _number.dispose();
    super.dispose();
  }

  Future<void> _decide(PendingDevice device, {required bool allow}) async {
    setState(() {
      _deciding = true;
      _error = null;
    });
    final error = await decideDevice(
      AppScope.of(context),
      device,
      allow: allow,
      number: _number.text,
    );
    if (!mounted) return;
    setState(() {
      _deciding = false;
      _error = error;
      if (error == null) _ended = allow ? 'Allowed.' : 'Not allowed.';
      // A wrong number: typed again from the start.
      if (error != null && allow) _number.clear();
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final device = widget.device;
    if (device == null) {
      return Text(
        _error ?? _ended ?? 'It stopped waiting.',
        style: type.caption.copyWith(
          color: _error == null ? palette.textFaint : palette.danger,
        ),
      );
    }
    final ready = !_deciding && isDeviceNumber(_number.text);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: Space.s,
      children: [
        Text(
          device.signedIn
              ? '${deviceCallingItself(device)}, signed in as you, wants to '
                    'connect.'
              : '${deviceCallingItself(device)} wants to connect.',
          style: type.body.copyWith(color: palette.text),
        ),
        Text(
          "If you didn't just set up a new device, don't allow it. Someone "
          'may be using your email: secure it.',
          style: type.body.copyWith(color: palette.textMuted),
        ),
        // Allowed on a phone or Mac, never here (approve_elsewhere.dart).
        if (approvesElsewhere)
          const ApproveElsewhere()
        else ...[
          Field(
            label: 'Number on its screen',
            child: Align(
              alignment: AlignmentDirectional.centerStart,
              child: SizedBox(
                width: DeviceNumberField.widthFor(context, least: 120),
                child: DeviceNumberField(
                  controller: _number,
                  onSubmitted: (_) {
                    if (ready) _decide(device, allow: true);
                  },
                ),
              ),
            ),
          ),
          if (_error case final error?)
            Text(error, style: type.caption.copyWith(color: palette.danger)),
          Wrap(
            alignment: WrapAlignment.end,
            spacing: Space.s,
            runSpacing: Space.s,
            children: [
              Button(
                label: "Don't allow",
                onPressed: _deciding
                    ? null
                    : () => _decide(device, allow: false),
              ),
              Button(
                label: 'Allow',
                primary: true,
                onPressed: ready ? () => _decide(device, allow: true) : null,
              ),
            ],
          ),
        ],
      ],
    );
  }
}
