import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'connections.dart';
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'tunnel_client.dart';
import 'settings_view.dart';
import 'usage.dart';
import 'tell_rivet.dart';

/// Settings → Devices: the devices that can show this Claktory, and adding
/// one. Any device showing it may add and remove devices: one already
/// added has full control. Adding one turns on access from other devices;
/// the backend turns it off again after the last is removed.
class DevicesSettings extends StatefulWidget {
  const DevicesSettings({super.key, required this.workspace});

  final Workspace workspace;

  @override
  State<DevicesSettings> createState() => _DevicesSettingsState();
}

class _DevicesSettingsState extends State<DevicesSettings>
    with SettingRequests {
  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final remote = widget.workspace.remote;
    final now = DateTime.now();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (backend.target case final target? when target.tunnelled) ...[
          SettingRow(
            label: 'This device',
            child: ValueListenableBuilder(
              valueListenable: backend.tunnel,
              builder: (context, status, _) => TunnelLine(
                status: status ?? idleTunnel,
                machine: target.label,
              ),
            ),
          ),
          const SizedBox(height: Space.m),
        ],
        if (remote.error.isNotEmpty) ...[
          ErrorLine(
            remote.error,
            about: 'Settings → Devices has an error.',
            style: TextStyles.body.copyWith(color: palette.danger),
          ),
          const SizedBox(height: Space.s),
        ],
        if (remote.devices.isEmpty)
          Text(
            'No devices yet.',
            style: TextStyles.body.copyWith(color: palette.textFaint),
          ),
        for (final device in remote.devices)
          _DeviceRow(
            device: device,
            thisDevice: device.id == backend.deviceId,
            machine: remote.machine,
            now: now,
          ),
        failureLine(palette),
        const SizedBox(height: Space.m),
        Row(
          children: [
            Button(
              label: 'Add a device…',
              onPressed: () => showModal(
                context,
                (context, close) => PairDeviceForm(close: close),
              ),
            ),
          ],
        ),
      ],
    );
  }
}

/// A device that can show this Claktory: its name, whether it's this one,
/// when it last connected and was added, and Remove.
class _DeviceRow extends StatefulWidget {
  const _DeviceRow({
    required this.device,
    required this.thisDevice,
    required this.machine,
    required this.now,
  });

  final PairedDevice device;

  /// The device this app runs on.
  final bool thisDevice;

  /// The machine this Factory runs on.
  final String machine;
  final DateTime now;

  @override
  State<_DeviceRow> createState() => _DeviceRowState();
}

class _DeviceRowState extends State<_DeviceRow> with SettingRequests {
  void _unpair() {
    final device = widget.device;
    void unpair() => run(() => AppScope.of(context).unpairDevice(device.id));
    if (!widget.thisDevice) return unpair();
    // Unpairing the device in hand cuts this app off at once.
    final machine = widget.machine.isEmpty ? 'this Claktory' : widget.machine;
    showModal(
      context,
      (context, close) => FormFrame(
        title: 'Remove this device?',
        body:
            'This app disconnects from $machine right away. To use it '
            'again, add it again on $machine.',
        fields: const [],
        error: null,
        submitLabel: 'Remove',
        onSubmit: () {
          close();
          unpair();
        },
        onCancel: close,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final device = widget.device;
    final seen = device.connected
        ? 'Connected now'
        : device.lastSeenMs == 0
        ? 'Never connected'
        : 'Last connected ${formatAge(device.lastSeenMs.toInt(), widget.now)}';
    final details = [
      seen,
      'Added ${pairedOn(device.pairedMs.toInt(), widget.now)}',
    ].join(' · ');
    return Container(
      padding: const EdgeInsets.symmetric(vertical: Space.s),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              SizedBox(
                width: Space.l,
                child: Align(
                  alignment: Alignment.centerLeft,
                  // Connected now: the live signal dot.
                  child: Dot(
                    color: device.connected
                        ? palette.signal
                        : palette.textFaint,
                  ),
                ),
              ),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text.rich(
                      TextSpan(
                        children: [
                          TextSpan(
                            text: device.name,
                            style: TextStyles.label.copyWith(
                              color: palette.text,
                            ),
                          ),
                          if (widget.thisDevice)
                            TextSpan(
                              text: '  This device',
                              style: TextStyles.caption.copyWith(
                                color: palette.textMuted,
                              ),
                            ),
                        ],
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      details,
                      style: TextStyles.caption.copyWith(
                        color: device.connected
                            ? palette.textMuted
                            : palette.textFaint,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: Space.m),
              Button(label: 'Remove', onPressed: _unpair),
            ],
          ),
          failureLine(palette),
        ],
      ),
    );
  }
}

/// When a device was added: "today", "yesterday", "12 Nov".
String pairedOn(int ms, DateTime now) {
  final day = DateTime.fromMillisecondsSinceEpoch(ms);
  final today = DateTime(now.year, now.month, now.day);
  final date = DateTime(day.year, day.month, day.day);
  if (date == today) return 'today';
  if (date == DateTime(today.year, today.month, today.day - 1)) {
    return 'yesterday';
  }
  const months = [
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
  ];
  final label = '${date.day} ${months[date.month - 1]}';
  return date.year == now.year ? label : '$label ${date.year}';
}

/// Adds a device: its name, then its code, shown this once.
class PairDeviceForm extends StatefulWidget {
  const PairDeviceForm({super.key, required this.close});

  final VoidCallback close;

  @override
  State<PairDeviceForm> createState() => _PairDeviceFormState();
}

class _PairDeviceFormState extends State<PairDeviceForm> {
  final _name = TextEditingController();
  var _submitting = false;
  String? _error;
  PairingCode? _code;

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final code = _code;
    if (code != null) {
      return PairingCodeView(
        code: code.code,
        device: _name.text.trim(),
        close: widget.close,
      );
    }
    return ListenableBuilder(
      listenable: _name,
      builder: (context, _) {
        final ready = !_submitting && _name.text.trim().isNotEmpty;
        return FormFrame(
          title: 'Add a device',
          error: _error,
          submitLabel: 'Show code',
          onSubmit: ready ? _add : null,
          onCancel: widget.close,
          fields: [
            Field(
              label: 'Name',
              child: TextField(
                controller: _name,
                placeholder: 'iPhone',
                autofocus: true,
                onSubmitted: (_) {
                  if (ready) _add();
                },
              ),
            ),
          ],
        );
      },
    );
  }

  Future<void> _add() async {
    final name = _name.text.trim();
    if (name.isEmpty || _submitting) return;
    setState(() {
      _submitting = true;
      _error = null;
    });
    final backend = AppScope.of(context);
    try {
      // A code waiting to be used is what lets the device in: the backend
      // turns remote access on by itself.
      final code = await backend.createPairing(name, '');
      if (mounted) setState(() => _code = code);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _submitting = false);
    }
  }
}

/// A new device's pairing code: a QR code and the link to copy, shown
/// only this once, since it holds the device's key.
class PairingCodeView extends StatefulWidget {
  const PairingCodeView({
    super.key,
    required this.code,
    required this.device,
    required this.close,
  });

  final String code;

  /// The device it's for, e.g. "Laptop".
  final String device;
  final VoidCallback close;

  @override
  State<PairingCodeView> createState() => _PairingCodeViewState();
}

class _PairingCodeViewState extends State<PairingCodeView> {
  var _copied = false;

  Future<void> _copy() async {
    await Clipboard.setData(ClipboardData(text: widget.code));
    if (mounted) setState(() => _copied = true);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final device = widget.device.isEmpty ? 'the device' : widget.device;
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.enter): widget.close},
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Scan with your iPhone',
            style: TextStyles.emphasis.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.s),
          Text(
            'Open Claktory on $device and scan this code.',
            style: TextStyles.body.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.xl),
          Center(child: QrCode(text: widget.code, size: 240)),
          const SizedBox(height: Space.l),
          Text(
            'Or copy the code and paste it on $device.',
            style: TextStyles.body.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.xs),
          Text(
            'This code works only for $device and is shown once.',
            style: TextStyles.caption.copyWith(color: palette.textFaint),
          ),
          const SizedBox(height: Space.xl),
          Row(
            children: [
              if (_copied)
                Text(
                  'Copied',
                  style: TextStyles.caption.copyWith(color: palette.textMuted),
                ),
              const Spacer(),
              Button(label: 'Copy code', onPressed: _copy),
              const SizedBox(width: Space.s),
              Button(label: 'Done', onPressed: widget.close, primary: true),
            ],
          ),
        ],
      ),
    );
  }
}

/// [text] as a QR code, dark modules on white with a quiet zone, drawn
/// from the modules the device makes; nothing where it can't make one.
class QrCode extends StatefulWidget {
  const QrCode({super.key, required this.text, required this.size});

  final String text;
  final double size;

  @override
  State<QrCode> createState() => _QrCodeState();
}

class _QrCodeState extends State<QrCode> {
  Future<QrModules?>? _modules;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _modules ??= AppScope.scope(context).connections.qrCode(widget.text);
  }

  @override
  Widget build(BuildContext context) => FutureBuilder<QrModules?>(
    future: _modules,
    builder: (context, snapshot) {
      final modules = snapshot.data;
      if (modules == null) return const SizedBox.shrink();
      return Semantics(
        image: true,
        label: 'QR code',
        child: CustomPaint(
          size: Size.square(widget.size),
          painter: _QrPainter(modules),
        ),
      );
    },
  );
}

class _QrPainter extends CustomPainter {
  _QrPainter(this.modules);

  final QrModules modules;

  @override
  void paint(Canvas canvas, Size size) {
    // White under it in either theme, with a four-module quiet zone, so
    // any camera reads it.
    const quiet = 4;
    // Whole pixels per module, so its edges are sharp; what's left over
    // goes around it.
    final across = modules.size + 2 * quiet;
    final cell = math.max(1.0, (size.width / across).floorToDouble());
    final inset = ((size.width - cell * across) / 2).floorToDouble();
    final rounded = RRect.fromRectAndRadius(
      Offset.zero & size,
      const Radius.circular(Radii.control),
    );
    canvas.drawRRect(rounded, Paint()..color = const Color(0xFFFFFFFF));
    final dark = Paint()..color = const Color(0xFF000000);
    final path = Path();
    for (var y = 0; y < modules.size; y++) {
      for (var x = 0; x < modules.size; x++) {
        if (!modules.at(x, y)) continue;
        path.addRect(
          Rect.fromLTWH(
            inset + (x + quiet) * cell,
            inset + (y + quiet) * cell,
            cell,
            cell,
          ),
        );
      }
    }
    canvas.drawPath(path, dark);
  }

  @override
  bool shouldRepaint(_QrPainter old) => old.modules != modules;
}

/// Which Factory this app shows: this Mac's, or one on another machine
/// it's paired with; and a new one from a pairing code. Opened from the
/// Factory name at the bottom of the sidebar.
class FactoryChooser extends StatefulWidget {
  const FactoryChooser({super.key, required this.close});

  final VoidCallback close;

  @override
  State<FactoryChooser> createState() => _FactoryChooserState();
}

class _FactoryChooserState extends State<FactoryChooser> {
  final _code = TextEditingController();

  @override
  void dispose() {
    _code.dispose();
    super.dispose();
  }

  void _show(RemoteFactory? target) {
    widget.close();
    AppScope.switchFactory(context, target);
  }

  @override
  Widget build(BuildContext context) {
    final connections = AppScope.scope(context).connections;
    final hasLocalBackend = AppScope.of(context).localBackend;
    return ListenableBuilder(
      listenable: Listenable.merge([connections, _code]),
      builder: (context, _) {
        final pasted = _code.text.trim();
        final target = parsePairingCode(pasted);
        final current = connections.current;
        return FormFrame(
          title: 'Your Claktories',
          error: pasted.isNotEmpty && target == null
              ? "That isn't a Claktory code."
              : null,
          submitLabel: 'Connect',
          onSubmit: target == null ? null : () => _show(target),
          onCancel: widget.close,
          fields: [
            if (hasLocalBackend)
              _FactoryRow(
                label: 'This Mac',
                detail: 'Running here',
                showing: current == null,
                onTap: () => _show(null),
              ),
            for (final saved in connections.saved)
              _FactoryRow(
                label: saved.label,
                detail: 'On another device',
                showing: saved == current,
                onTap: () => _show(saved),
                onForget: () {
                  final wasShowing = saved == current;
                  connections.forget(saved);
                  if (wasShowing) AppScope.switchFactory(context, null);
                },
              ),
            const SizedBox(height: Space.l),
            Field(
              label: 'Connect to another Claktory',
              note: "Paste the code from that Claktory's Settings → Devices.",
              child: TextField(
                controller: _code,
                placeholder: 'Paste code',
                autofocus: true,
                onSubmitted: (_) {
                  if (target != null) _show(target);
                },
              ),
            ),
          ],
        );
      },
    );
  }
}

/// One Factory in the chooser: its name and address, the signal dot on the
/// one showing, and Forget for one on another machine.
class _FactoryRow extends StatelessWidget {
  const _FactoryRow({
    required this.label,
    required this.detail,
    required this.showing,
    required this.onTap,
    this.onForget,
  });

  final String label;
  final String detail;
  final bool showing;
  final VoidCallback onTap;
  final VoidCallback? onForget;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      selected: showing,
      label: label,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => AnimatedContainer(
          duration: Motion.fast,
          padding: const EdgeInsets.symmetric(
            horizontal: Space.s,
            vertical: Space.s,
          ),
          decoration: BoxDecoration(
            color: showing
                ? palette.selected
                : hovered
                ? palette.hover
                : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Row(
            children: [
              SizedBox(
                width: Space.l,
                child: showing ? Dot(color: palette.signal) : null,
              ),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      label,
                      style: TextStyles.label.copyWith(color: palette.text),
                    ),
                    Text(
                      detail,
                      style: TextStyles.caption.copyWith(
                        color: palette.textFaint,
                      ),
                    ),
                  ],
                ),
              ),
              if (onForget != null && hovered)
                Button(label: 'Forget', onPressed: onForget),
            ],
          ),
        ),
      ),
    );
  }
}

/// What this device is: "iPhone" or "Mac", for what it's told.
String get thisDeviceKind =>
    defaultTargetPlatform == TargetPlatform.iOS ? 'iPhone' : 'Mac';

/// Whether this device is connected to [machine], connecting, or why it
/// can't connect.
class TunnelLine extends StatelessWidget {
  const TunnelLine({super.key, required this.status, required this.machine});

  final TunnelStatus status;
  final String machine;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final connected = status.phase == TunnelPhase.connected;
    final failed = status.failure != TunnelFailure.none && !connected;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          failed
              ? "Can't connect to $machine"
              : connected
              ? 'Connected to $machine'
              : 'Connecting to $machine…',
          style: TextStyles.body.copyWith(
            color: failed ? palette.danger : palette.textMuted,
          ),
        ),
        Text(
          failed
              ? tunnelAdvice(status, machine, device: thisDeviceKind)
              : 'Private and encrypted.',
          style: TextStyles.caption.copyWith(
            color: failed ? palette.textMuted : palette.textFaint,
          ),
        ),
      ],
    );
  }
}

/// Why the app can't connect to the Claktory it shows, and what to do:
/// opened from the sidebar's footer.
class TunnelProblem extends StatelessWidget {
  const TunnelProblem({super.key, required this.close});

  final VoidCallback close;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final machine = backend.target?.label ?? 'this Claktory';
    return ValueListenableBuilder(
      valueListenable: backend.tunnel,
      builder: (context, status, _) {
        final now = status ?? idleTunnel;
        final connected = now.phase == TunnelPhase.connected;
        return FormFrame(
          title: connected
              ? 'Connected to $machine'
              : "Can't connect to $machine",
          body: connected
              ? 'Private and encrypted.'
              : tunnelAdvice(now, machine, device: thisDeviceKind),
          fields: [
            // What went wrong exactly, for whoever looks into it.
            if (!connected && now.error.isNotEmpty)
              Padding(
                padding: const EdgeInsets.only(bottom: Space.m),
                child: Text(
                  now.error,
                  style: TextStyles.caption.copyWith(color: palette.textFaint),
                ),
              ),
          ],
          error: null,
          submitLabel: 'Try again',
          onSubmit: connected
              ? null
              : () {
                  backend.retry();
                  close();
                },
          onCancel: close,
          secondary: Button(
            label: 'Switch Claktory…',
            onPressed: () {
              close();
              showModal(
                context,
                (context, close) => FactoryChooser(close: close),
              );
            },
          ),
        );
      },
    );
  }
}
