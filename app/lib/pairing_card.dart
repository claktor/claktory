import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'avatar.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'devices.dart' show QrCode;
import 'extensions.dart' show logTime;
import 'identity.dart';
import 'pairing.dart' show PairingDigits;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// An agent adding one of the person's devices (Devices extension), in its
/// conversation: while it waits, the code to scan or copy, which only this
/// app gets (ShowPairingCode) and keeps only while the card shows, and how
/// long it still works; once a device used it, the number to compare and
/// Allow; then how it ended. A sibling of the secret request card.
class DevicePairingCard extends StatefulWidget {
  const DevicePairingCard(this.pairing, {super.key});

  final DevicePairing pairing;

  @override
  State<DevicePairingCard> createState() => _DevicePairingCardState();
}

class _DevicePairingCardState extends State<DevicePairingCard> {
  Backend? _backend;

  /// The code, while it waits and has arrived.
  PairingCode? _code;

  /// Counts the asks for the code, so only the latest answer shows.
  var _asked = 0;

  /// Ticks the time left while it waits.
  Timer? _tick;
  var _copied = false;

  /// Cancel, Allow or Don't allow is on its way.
  var _deciding = false;
  String? _error;

  DevicePairing get pairing => widget.pairing;

  bool get _waiting =>
      pairing.state == DevicePairingState.DEVICE_PAIRING_STATE_WAITING;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (backend == _backend) return;
    _backend?.connected.removeListener(_connectedChanged);
    _backend = backend..connected.addListener(_connectedChanged);
    _code = null;
    _sync();
  }

  @override
  void didUpdateWidget(DevicePairingCard old) {
    super.didUpdateWidget(old);
    if (old.pairing.state != pairing.state || old.pairing.id != pairing.id) {
      _error = null;
      _sync();
    }
  }

  @override
  void dispose() {
    _backend?.connected.removeListener(_connectedChanged);
    _tick?.cancel();
    super.dispose();
  }

  /// Asks for the code and ticks while it waits; forgets it once it
  /// doesn't.
  void _sync() {
    if (_waiting) {
      if (_code == null) unawaited(_fetch());
      _tick ??= Timer.periodic(const Duration(seconds: 1), (_) {
        if (mounted) setState(() {});
      });
    } else {
      _asked++;
      _code = null;
      _copied = false;
      _tick?.cancel();
      _tick = null;
    }
  }

  /// Connected again: a backend that restarted has a new code, which
  /// replaces the one shown.
  void _connectedChanged() {
    if (_backend?.connected.value == true && _waiting) unawaited(_fetch());
  }

  Future<void> _fetch() async {
    final backend = _backend;
    if (backend == null) return;
    final asked = ++_asked;
    try {
      final code = await backend.showPairingCode(pairing.id);
      if (!mounted || asked != _asked || !_waiting) return;
      setState(() {
        if (_code?.code != code.code) _copied = false;
        _code = code;
        _error = null;
      });
    } on RequestFailure catch (failure) {
      // No longer waiting: its new state is on its way.
      if (failure.code == ErrorCode.ERROR_CODE_NOT_FOUND) return;
      if (mounted && asked == _asked) {
        setState(() => _error = failure.message);
      }
    }
  }

  Future<void> _copy() async {
    final code = _code;
    if (code == null) return;
    await Clipboard.setData(ClipboardData(text: code.code));
    if (mounted) setState(() => _copied = true);
  }

  Future<void> _decide(Future<void> Function(Backend backend) send) async {
    final backend = _backend;
    if (_deciding || backend == null) return;
    setState(() {
      _deciding = true;
      _error = null;
    });
    try {
      await send(backend);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _deciding = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => _card(
        context,
        store.agent(pairing.agentId),
        store.workspace.remote.pendingDevices
            .where((device) => device.deviceId == pairing.deviceId)
            .firstOrNull,
      ),
    );
  }

  Widget _card(BuildContext context, Agent? agent, PendingDevice? asking) {
    final palette = Palette.of(context);
    final color = agentColor(agent?.color ?? 'slate');
    final name = agent?.name ?? pairing.agentId;
    final device = pairing.name.isEmpty ? 'the device' : pairing.name;
    final open =
        _waiting ||
        pairing.state == DevicePairingState.DEVICE_PAIRING_STATE_ASKING;
    return Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.06),
        border: Border.all(color: color.withValues(alpha: open ? 0.55 : 0.25)),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              if (agent != null) ...[
                AgentCharacter(agent: agent, size: 20),
                const SizedBox(width: Space.xs + 2),
              ],
              Flexible(
                child: Text(
                  '$name adds a device: ${pairing.name}',
                  style: TextStyles.caption.copyWith(
                    color: color,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.m),
          if (_waiting)
            ..._waitingParts(palette, device)
          else if (pairing.state ==
              DevicePairingState.DEVICE_PAIRING_STATE_ASKING)
            ..._askingParts(palette, device, asking)
          else
            _ended(palette, color, device),
        ],
      ),
    );
  }

  List<Widget> _waitingParts(Palette palette, String device) {
    final code = _code;
    final left = Duration(
      milliseconds:
          pairing.expiresMs.toInt() - pairingNow().millisecondsSinceEpoch,
    );
    final body = TextStyles.body.copyWith(color: palette.textMuted);
    return [
      Text('Open Claktory on $device and scan this code.', style: body),
      const SizedBox(height: Space.l),
      Center(
        child: code == null
            // Its room, until the code arrives.
            ? const SizedBox.square(dimension: _qrSize)
            : QrCode(key: ValueKey(code.code), text: code.code, size: _qrSize),
      ),
      const SizedBox(height: Space.l),
      Text('Or copy the code and paste it on $device.', style: body),
      const SizedBox(height: Space.xs),
      Text(
        _error ??
            'This code works only for $device. It expires in '
                '${countdown(left)}.',
        style: TextStyles.caption.copyWith(
          color: _error == null ? palette.textFaint : palette.danger,
        ),
      ),
      const SizedBox(height: Space.l),
      Row(
        mainAxisAlignment: MainAxisAlignment.end,
        children: [
          Button(
            label: _copied ? 'Copied' : 'Copy code',
            onPressed: code == null ? null : _copy,
          ),
          const SizedBox(width: Space.s),
          Button(
            label: 'Cancel',
            onPressed: _deciding
                ? null
                : () => _decide(
                    (backend) => backend.cancelPairing(pairing.deviceId),
                  ),
          ),
        ],
      ),
    ];
  }

  List<Widget> _askingParts(
    Palette palette,
    String device,
    PendingDevice? asking,
  ) {
    return [
      Text(
        asking == null || asking.number.isEmpty
            ? '$device used the code.'
            : '$device used the code. Allow it only if its screen shows '
                  'the same number.',
        style: TextStyles.body.copyWith(color: palette.textMuted),
      ),
      if (asking != null && asking.number.isNotEmpty) ...[
        const SizedBox(height: Space.l),
        PairingDigits(asking.number),
      ],
      const SizedBox(height: Space.l),
      Row(
        children: [
          Expanded(
            child: Text(
              _error ?? '',
              style: TextStyles.caption.copyWith(color: palette.danger),
            ),
          ),
          const SizedBox(width: Space.s),
          Button(
            label: "Don't allow",
            onPressed: _deciding || asking == null
                ? null
                : () =>
                      _decide((backend) => backend.denyDevice(asking.deviceId)),
          ),
          const SizedBox(width: Space.s),
          Button(
            label: 'Allow',
            primary: true,
            onPressed: _deciding || asking == null
                ? null
                : () => _decide(
                    (backend) => backend.approveDevice(asking.deviceId),
                  ),
          ),
        ],
      ),
    ];
  }

  /// How it ended, and when.
  Widget _ended(Palette palette, Color color, String device) {
    final (text, byPerson) = switch (pairing.state) {
      DevicePairingState.DEVICE_PAIRING_STATE_ADDED => (
        '$device was added.',
        true,
      ),
      DevicePairingState.DEVICE_PAIRING_STATE_DENIED => (
        "You didn't allow $device.",
        true,
      ),
      DevicePairingState.DEVICE_PAIRING_STATE_CANCELLED => (
        'Cancelled.',
        false,
      ),
      DevicePairingState.DEVICE_PAIRING_STATE_EXPIRED => (
        'The code expired before $device used it.',
        false,
      ),
      _ => ('', false),
    };
    final added =
        pairing.state == DevicePairingState.DEVICE_PAIRING_STATE_ADDED;
    final ended = pairing.endedMs.toInt();
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (byPerson) ...[
          const PersonAvatar(size: 16),
          const SizedBox(width: Space.xs + 2),
        ],
        Flexible(
          child: Text.rich(
            TextSpan(
              children: [
                TextSpan(
                  text: text,
                  style: TextStyle(
                    color: added ? color : palette.textMuted,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                if (ended > 0)
                  TextSpan(
                    text:
                        ' · ${logTime(DateTime.fromMillisecondsSinceEpoch(ended))}',
                    style: TextStyle(color: palette.textFaint),
                  ),
              ],
            ),
            style: TextStyles.caption,
          ),
        ),
      ],
    );
  }
}

/// The QR code's side on the card: smaller than Add a device's, still easy
/// to scan from a phone held to the screen.
const _qrSize = 200.0;

/// The time the countdown counts from; tests set it.
@visibleForTesting
DateTime Function() pairingNow = DateTime.now;

/// [left] as minutes and seconds, "9:42"; never below "0:00".
String countdown(Duration left) {
  final seconds = left.isNegative ? 0 : (left.inMilliseconds / 1000).ceil();
  return '${seconds ~/ 60}:${(seconds % 60).toString().padLeft(2, '0')}';
}
