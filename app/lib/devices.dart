import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'android/device_identity.dart';
import 'backend.dart';
import 'claktory_identity.dart' show claktoryName;
import 'connections.dart';
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'links.dart' show confirmPairing, pairingFingerprint;
import 'pasted_code.dart';
import 'mobile/device.dart' show deviceCapabilities;
import 'mobile/drawer.dart' show largeText;
import 'mobile/metrics.dart' show TypeScale;
import 'mobile/input.dart' show InputKind;
import 'protocol/factory/v1/factory.pb.dart';
import 'phone.dart' show pairingProblem;
import 'rail.dart' show ClaktoriesScope;
import 'scan_code.dart';
import 'scope.dart';
import 'tunnel_client.dart';
import 'settings_view.dart';
import 'usage.dart';
import 'tell_rivet.dart';
import 'visible_codes.dart' show proseCodes;
import 'your_claktories.dart'
    show YourClaktories, deadlineDay, showConfirmSignIn;

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
            style: TypeScale.of(context).body.copyWith(color: palette.danger),
          ),
          const SizedBox(height: Space.s),
        ],
        if (remote.devices.isEmpty)
          Text(
            'No devices yet.',
            style: TypeScale.of(context).body
                .copyWith(color: palette.textFaint),
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
        Align(
          alignment: Alignment.centerLeft,
          child: Button(
            label: 'Add a device…',
            onPressed: () => showModal(
              context,
              (context, close) => PairDeviceForm(close: close),
            ),
          ),
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
    // Not shown to be the owner's yet: by when it has to be, or that it's
    // past that and turned away. Before it connected with an app that can
    // confirm, it isn't asked to.
    final confirmBy = device.confirmed || device.confirmByMs == 0
        ? null
        : DateTime.fromMillisecondsSinceEpoch(device.confirmByMs.toInt());
    final late = confirmBy != null && !confirmBy.isAfter(widget.now);
    // On a phone two buttons don't fit beside the words: they go under them,
    // as they do on any device at large text sizes (an iPad's left the
    // words a sliver, Zucchero's row 133).
    final stacked =
        largeText(context) || (isPhoneLayout(context) && confirmBy != null);
    final actions = [
      if (confirmBy != null)
        Button(
          label: widget.thisDevice ? 'Confirm…' : "Confirm it's yours",
          onPressed: widget.thisDevice
              ? () => showConfirmSignIn(
                  context,
                  claktoryName(AppScope.of(context)),
                )
              : () => run(() => AppScope.of(context).confirmDevice(device.id)),
        ),
      Button(label: 'Remove', onPressed: _unpair),
    ];
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
                            style: TypeScale.of(context).label
                                .copyWith(color: palette.text),
                          ),
                          if (widget.thisDevice)
                            TextSpan(
                              text: '  This device',
                              style: TypeScale.of(context).caption
                                  .copyWith(color: palette.textMuted),
                            ),
                        ],
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      details,
                      style: TypeScale.of(context).caption.copyWith(
                        color: device.connected
                            ? palette.textMuted
                            : palette.textFaint,
                      ),
                    ),
                    if (confirmBy != null)
                      Text(
                        late
                            ? "Not confirmed: it can't connect until it's "
                                  'confirmed'
                            : 'Not confirmed yet · confirm by '
                                  '${deadlineDay(confirmBy, widget.now)}',
                        style: TypeScale.of(context).caption.copyWith(
                          color: late ? palette.danger : palette.textMuted,
                        ),
                      ),
                  ],
                ),
              ),
              if (!stacked)
                for (final (i, action) in actions.indexed) ...[
                  SizedBox(width: i == 0 ? Space.m : Space.s),
                  action,
                ],
            ],
          ),
          // At large text sizes the buttons go under the words.
          if (stacked)
            Padding(
              padding: const EdgeInsets.only(left: Space.l, top: Space.s),
              child: Wrap(
                spacing: Space.s,
                runSpacing: Space.s,
                children: actions,
              ),
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

  /// What the device is, as the person says: a phone or tablet scans its
  /// code, a computer pastes it.
  var _kind = DeviceKind.DEVICE_KIND_PHONE;
  var _submitting = false;

  /// The connection dropped while the code was asked for: waiting for it.
  var _reconnecting = false;
  String? _error;
  PairingCode? _code;

  /// The device was let in: nothing to cancel as the form closes.
  var _allowed = false;

  /// Allow or Don't allow is on its way.
  var _deciding = false;

  late final Backend _backend = AppScope.of(context);

  @override
  void dispose() {
    _name.dispose();
    // A code lasts only while it's shown: closing ends it, unless it let
    // its device in (then this does nothing).
    final code = _code;
    if (code != null && !_allowed) {
      fireAndLog(_backend.cancelPairing(code.deviceId));
    }
    super.dispose();
  }

  Future<void> _decide(PendingDevice device, {required bool allow}) async {
    setState(() {
      _deciding = true;
      _error = null;
    });
    try {
      if (allow) {
        await _backend.approveDevice(device.deviceId);
        _allowed = true;
      } else {
        await _backend.denyDevice(device.deviceId);
      }
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(
          () => _error = failure.code == ErrorCode.ERROR_CODE_NOT_FOUND
              ? '${device.name} stopped waiting. To add it, make a new code.'
              : failure.message,
        );
      }
    } finally {
      if (mounted) setState(() => _deciding = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final code = _code;
    if (code != null) {
      return ListenableBuilder(
        listenable: _backend.workspace,
        builder: (context, _) {
          final remote = _backend.workspace.workspace.remote;
          final waiting = remote.pendingDevices
              .where((device) => device.deviceId == code.deviceId)
              .firstOrNull;
          // The device used the code: let it in, or not.
          if (waiting != null || _deciding || _error != null) {
            final device = waiting ?? PendingDevice(name: _name.text.trim());
            return FormFrame(
              // The name the device gave itself: what reorders text shows
              // by its code (proseCodes, Fugo).
              title: 'Allow ${proseCodes(device.name)} to connect?',
              body: device.number.isEmpty
                  ? null
                  : 'Check that its screen shows ${device.number}.',
              error: _error,
              fields: const [],
              submitLabel: 'Allow',
              onSubmit: waiting == null || _deciding
                  ? null
                  : () => _decide(waiting, allow: true),
              cancelLabel: waiting == null ? 'Close' : "Don't allow",
              onCancel: waiting == null || _deciding
                  ? widget.close
                  : () => _decide(waiting, allow: false),
            );
          }
          return PairingCodeView(
            code: code.code,
            device: _name.text.trim(),
            way: pairingWayOf(_kind),
            close: widget.close,
          );
        },
      );
    }
    return ListenableBuilder(
      listenable: _name,
      builder: (context, _) {
        final ready = !_submitting && _name.text.trim().isNotEmpty;
        return FormFrame(
          title: 'Add a device',
          body: _reconnecting
              ? 'Preparing the code… This takes a few seconds the first '
                    'time.'
              : null,
          error: _error,
          submitLabel: 'Show code',
          onSubmit: ready ? _add : null,
          onCancel: widget.close,
          fields: [
            Field(
              label: 'Device',
              child: Select<DeviceKind>(
                title: 'Device',
                value: _kind,
                options: const [
                  SelectOption(DeviceKind.DEVICE_KIND_PHONE, 'Phone or tablet'),
                  SelectOption(DeviceKind.DEVICE_KIND_DESKTOP, 'Computer'),
                ],
                onChanged: (kind) => setState(() => _kind = kind),
              ),
            ),
            Field(
              label: 'Name',
              child: TextField(
                controller: _name,
                placeholder: _kind == DeviceKind.DEVICE_KIND_PHONE
                    ? 'iPhone'
                    : 'MacBook',
                kind: InputKind.text,
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
    final started = DateTime.now().millisecondsSinceEpoch;
    try {
      // A code waiting to be used is what lets the device in: the backend
      // turns remote access on by itself.
      PairingCode code;
      try {
        code = await backend.createPairing(name, '');
      } on RequestFailure {
        if (backend.connected.value) rethrow;
        // The first device turns access from other devices on, which
        // restarts the tunnel this app reaches its own Claktory through:
        // the connection drops before the code arrives. Wait for it to
        // come back, take back the device the lost answer made, and ask
        // again.
        if (mounted) setState(() => _reconnecting = true);
        if (!await _reconnected(backend)) rethrow;
        for (final device in backend.workspace.workspace.remote.devices) {
          if (device.name == name &&
              device.lastSeenMs == 0 &&
              device.codeExpiresMs > 0 &&
              device.pairedMs.toInt() >= started - 5000) {
            await backend.unpairDevice(device.id);
          }
        }
        code = await backend.createPairing(name, '');
      }
      if (mounted) setState(() => _code = code);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) {
        setState(() {
          _submitting = false;
          _reconnecting = false;
        });
      }
    }
  }

  /// Waits up to [timeout] for [backend] to connect again and send its
  /// state; false if it doesn't.
  static Future<bool> _reconnected(
    Backend backend, {
    Duration timeout = const Duration(seconds: 45),
  }) async {
    final done = Completer<bool>();
    void connected() {
      if (backend.connected.value && !done.isCompleted) done.complete(true);
    }

    backend.connected.addListener(connected);
    final timer = Timer(timeout, () {
      if (!done.isCompleted) done.complete(false);
    });
    connected();
    final ok = await done.future;
    backend.connected.removeListener(connected);
    timer.cancel();
    if (!ok) return false;
    // Its state follows its welcome: wait for it, briefly.
    final state = Completer<void>();
    void received() {
      if (!state.isCompleted) state.complete();
    }

    backend.workspace.addListener(received);
    await state.future.timeout(const Duration(seconds: 3), onTimeout: () {});
    backend.workspace.removeListener(received);
    return backend.connected.value;
  }
}

/// How a new device takes its code: a phone or tablet scans it with its
/// camera; a computer can't, so its code is copied and pasted there.
enum PairingWay {
  /// A phone or tablet: the QR code first, Copy code too.
  scan,

  /// A computer: Copy code first; the QR code only when asked for.
  paste,

  /// Not said (an agent's card that gave no kind): Copy code first, the
  /// QR code under it for a phone or tablet.
  either,
}

/// The way a [kind] of device takes its code.
PairingWay pairingWayOf(DeviceKind kind) => switch (kind) {
  DeviceKind.DEVICE_KIND_PHONE ||
  DeviceKind.DEVICE_KIND_TABLET => PairingWay.scan,
  DeviceKind.DEVICE_KIND_DESKTOP ||
  DeviceKind.DEVICE_KIND_LAPTOP => PairingWay.paste,
  _ => PairingWay.either,
};

/// What to do with a new device's code, the [way] its device takes it:
/// what to do on [device], and the QR code where it helps. [code] is null
/// until it arrives (its room is kept).
class PairingInstructions extends StatefulWidget {
  const PairingInstructions({
    super.key,
    required this.code,
    required this.device,
    required this.way,
    required this.qrSize,
  });

  final String? code;
  final String device;
  final PairingWay way;
  final double qrSize;

  @override
  State<PairingInstructions> createState() => _PairingInstructionsState();
}

class _PairingInstructionsState extends State<PairingInstructions> {
  /// A computer's QR code, shown once asked for.
  var _qrShown = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final body = TypeScale.of(context).body.copyWith(color: palette.textMuted);
    final device = widget.device;
    final code = widget.code;
    final target = code == null ? null : parsePairingCode(code);
    final qr = Column(
      children: [
        Center(
          child: code == null
              ? SizedBox.square(dimension: widget.qrSize)
              : QrCode(key: ValueKey(code), text: code, size: widget.qrSize),
        ),
        // What the other device shows before it connects, to compare.
        if (target != null) ...[
          const SizedBox(height: Space.s),
          Text.rich(
            TextSpan(
              children: [
                const TextSpan(text: 'Its code: '),
                TextSpan(
                  text: pairingFingerprint(target),
                  style: TextStyle(
                    fontFamily: TextStyles.mono,
                    color: palette.text,
                  ),
                ),
              ],
            ),
            textAlign: TextAlign.center,
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textMuted),
          ),
        ],
      ],
    );
    final paste = Text(
      'In Claktory on $device, click Add a Claktory and paste this code.',
      style: body,
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: switch (widget.way) {
        PairingWay.scan => [
          Text('Open Claktory on $device and scan this code.', style: body),
          const SizedBox(height: Space.l),
          qr,
          const SizedBox(height: Space.l),
          Text('Or copy the code and paste it on $device.', style: body),
        ],
        PairingWay.paste => [
          paste,
          if (_qrShown) ...[
            const SizedBox(height: Space.l),
            qr,
          ] else ...[
            const SizedBox(height: Space.s),
            Align(
              alignment: AlignmentDirectional.centerStart,
              child: Button(
                label: 'Show QR code',
                onPressed: () => setState(() => _qrShown = true),
              ),
            ),
          ],
        ],
        PairingWay.either => [
          paste,
          const SizedBox(height: Space.l),
          qr,
          const SizedBox(height: Space.l),
          Text(
            'On a phone or tablet, open Claktory and scan it instead.',
            style: body,
          ),
        ],
      },
    );
  }
}

/// A new device's pairing code, shown only this once, since it holds the
/// device's key: what to do with it on the device, the way it takes it
/// ([PairingInstructions]), and Copy code, first for a computer.
class PairingCodeView extends StatefulWidget {
  const PairingCodeView({
    super.key,
    required this.code,
    required this.device,
    this.way = PairingWay.scan,
    required this.close,
  });

  final String code;

  /// The device it's for, e.g. "Laptop".
  final String device;

  /// How the device takes the code.
  final PairingWay way;
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
    // A computer can't scan: copying is what it takes.
    final copyFirst = widget.way != PairingWay.scan;
    final copy = Button(
      label: 'Copy code',
      primary: copyFirst,
      onPressed: _copy,
    );
    // Closing ends the code: it lasts only while it's shown.
    final cancel = Button(label: 'Cancel', onPressed: widget.close);
    final copied = Text(
      'Copied',
      style: TypeScale.of(context).caption.copyWith(color: palette.textMuted),
    );
    return CallbackShortcuts(
      // Escape closes it, as every form; Enter doesn't, since closing ends
      // the code.
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): widget.close,
      },
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Connect $device',
            style: TypeScale.of(context).emphasis.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.s),
          PairingInstructions(
            code: widget.code,
            device: device,
            way: widget.way,
            qrSize: 240,
          ),
          const SizedBox(height: Space.xs),
          Text(
            'This code works only for $device and is shown once.',
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textFaint),
          ),
          const SizedBox(height: Space.xl),
          if (isPhoneLayout(context) && largeText(context))
            // No room for both side by side: each on a line of its own.
            Wrap(
              spacing: Space.s,
              runSpacing: Space.s,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [copy, cancel, if (_copied) copied],
            )
          else
            Row(
              children: [
                if (_copied) copied,
                const Spacer(),
                if (copyFirst) ...[
                  cancel,
                  const SizedBox(width: Space.s),
                  copy,
                ] else ...[
                  copy,
                  const SizedBox(width: Space.s),
                  cancel,
                ],
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

  /// Whether Scan code shows beside the field: where there's a camera
  /// (friend bug #2).
  var _canScan = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      if (!mounted) return;
      final can = await canOfferScan(context);
      if (mounted && can != _canScan) setState(() => _canScan = can);
    });
  }

  /// Scans the other Claktory's code, which then connects as a pasted one
  /// does, once the person says so.
  Future<void> _scan() async {
    final scanned = await scanCode(
      context,
      title: 'Scan code',
      check: (text) => pairingProblem(text, scanned: true),
    );
    if (scanned == null || !mounted) return;
    _code.text = scanned;
    if (pastedPairingCode(scanned) case final target?) {
      await _connectPasted(target);
    }
  }

  @override
  void dispose() {
    _code.dispose();
    super.dispose();
  }

  void _show(RemoteFactory? target) {
    widget.close();
    AppScope.switchFactory(context, target);
  }

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
    _show(target);
  }

  /// Whether [confirmPairing] is asking about a pasted code.
  var _asking = false;

  @override
  Widget build(BuildContext context) {
    final connections = AppScope.scope(context).connections;
    // This Mac's own Claktory, if it runs one (else it can be set up).
    final claktories = ClaktoriesScope.of(context);
    final hasLocalBackend =
        claktories?.localBackend ?? AppScope.of(context).localBackend;
    return ListenableBuilder(
      listenable: Listenable.merge([connections, _code]),
      builder: (context, _) {
        final pasted = _code.text.trim();
        final target = pastedPairingCode(pasted);
        final current = connections.current;
        return FormFrame(
          title: 'Your Claktories',
          error: pasted.isNotEmpty && target == null
              ? otherCodeProblem(
                      pasted,
                      device: thisDeviceKind,
                      // An iPhone's or iPad's Claktory is on a Mac, as the phone's
                      // pairing screen says (Zucchero's row 219).
                      where: defaultTargetPlatform == TargetPlatform.iOS
                          ? onMac
                          : whereItRuns,
                    ) ??
                    "That isn't a Claktory code."
              : null,
          submitLabel: 'Connect',
          onSubmit: target == null
              ? null
              : () => unawaited(_connectPasted(target)),
          onCancel: widget.close,
          fields: [
            if (hasLocalBackend)
              _FactoryRow(
                label: 'This Mac',
                detail: 'Running here',
                showing: current == null,
                onTap: () => _show(null),
              )
            else if (claktories != null && claktories.canChooseLocal)
              _FactoryRow(
                label: 'This Mac',
                detail: 'Set up Claktory here',
                showing: false,
                onTap: () {
                  widget.close();
                  claktories.setLocalBackend(true);
                },
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
              label: 'On your account',
              child: YourClaktories(onConnect: _show, hideConnected: true),
            ),
            const SizedBox(height: Space.l),
            Field(
              label: 'Connect to another Claktory',
              note: _canScan
                  ? "Scan or paste the code from that Claktory's Settings → "
                        'Devices, or the one `factoryd pair` prints on a '
                        'server.'
                  : "Paste the code from that Claktory's Settings → Devices, "
                        'or the one `factoryd pair` prints on a server.',
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  TextField(
                    controller: _code,
                    placeholder: 'Paste code',
                    autocorrect: false,
                    textCapitalization: TextCapitalization.none,
                    autofocus: true,
                    onSubmitted: (_) {
                      if (target != null) unawaited(_connectPasted(target));
                    },
                  ),
                  if (_canScan) ...[
                    const SizedBox(height: Space.s),
                    Align(
                      alignment: AlignmentDirectional.centerStart,
                      child: Button(
                        label: 'Scan code',
                        onPressed: () => unawaited(_scan()),
                      ),
                    ),
                  ],
                ],
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
                      style: TypeScale.of(context).label
                          .copyWith(color: palette.text),
                    ),
                    Text(
                      detail,
                      style: TypeScale.of(context).caption
                          .copyWith(color: palette.textFaint),
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

/// What this device is, for what it's told: "iPhone", "Mac", or on
/// Android "phone" or "tablet" (from the display, not the window).
String get thisDeviceKind => switch (defaultTargetPlatform) {
  TargetPlatform.iOS =>
    deviceCapabilities.idiom == Idiom.pad ? 'iPad' : 'iPhone',
  TargetPlatform.android => AndroidDeviceIdentity.forDisplayShortestSide(
    androidDisplayShortestSide(),
  ).noun,
  _ => 'Mac',
};

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
          style: TypeScale.of(context).body
              .copyWith(color: failed ? palette.danger : palette.textMuted),
        ),
        Text(
          failed
              ? tunnelAdvice(status, machine, device: thisDeviceKind)
              : 'Private and encrypted.',
          style: TypeScale.of(context).caption
              .copyWith(color: failed ? palette.textMuted : palette.textFaint),
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
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textFaint),
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
