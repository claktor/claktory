import 'dart:io';

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

/// Settings → Devices: remote access to this Factory, and the devices
/// paired with it. Any device showing this Factory may pair and unpair: a
/// paired device already has full control.
class DevicesSettings extends StatefulWidget {
  const DevicesSettings({super.key, required this.workspace});

  final Workspace workspace;

  @override
  State<DevicesSettings> createState() => _DevicesSettingsState();
}

class _DevicesSettingsState extends State<DevicesSettings>
    with SettingRequests {
  final _port = TextEditingController();
  var _shownPort = 0;

  @override
  void initState() {
    super.initState();
    _syncPort();
  }

  @override
  void didUpdateWidget(DevicesSettings old) {
    super.didUpdateWidget(old);
    _syncPort();
  }

  /// Shows the backend's port unless the person is editing it.
  void _syncPort() {
    final port = widget.workspace.remote.port;
    if (port == _shownPort) return;
    final editing = _port.text != '$_shownPort' && _port.text.isNotEmpty;
    _shownPort = port;
    if (!editing) _port.text = port == 0 ? '' : '$port';
  }

  @override
  void dispose() {
    _port.dispose();
    super.dispose();
  }

  void _savePort() {
    final port = int.tryParse(_port.text.trim());
    if (port == null || port < 1 || port > 65535) {
      setState(() => failure = 'Enter a port from 1 to 65535, like 7443.');
      return;
    }
    run(() => AppScope.of(context).setRemoteAccess(true, port: port));
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final remote = widget.workspace.remote;
    // Only IP addresses can be listened on; names (mac-mini.local) can't.
    final listenable = [
      for (final address in remote.addresses)
        if (InternetAddress.tryParse(address) != null) address,
      if (remote.listen.isNotEmpty && !remote.addresses.contains(remote.listen))
        remote.listen,
    ];
    final now = DateTime.now();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SettingRow(
          label: 'Remote access',
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Padding(
                padding: const EdgeInsets.only(top: 2),
                child: Toggle(
                  value: remote.on,
                  label: 'Remote access',
                  onChanged: (on) => run(() => backend.setRemoteAccess(on)),
                ),
              ),
              const SizedBox(width: Space.m),
              Expanded(
                child: Text(
                  'Your other devices, such as a laptop, can show this '
                  'Claktory and work with its agents, over the network. '
                  'Pair each one below.',
                  style: TextStyles.caption.copyWith(color: palette.textMuted),
                ),
              ),
            ],
          ),
        ),
        if (remote.on) ...[
          SettingRow(
            label: 'Status',
            child: remote.error.isNotEmpty
                ? ErrorLine(
                    remote.error,
                    about: 'Remote access (Settings → Devices) has an error.',
                    style: TextStyles.body.copyWith(color: palette.danger),
                  )
                : Text(
                    remote.fingerprint.isEmpty
                        ? 'Starting…'
                        : remote.listen.isEmpty
                        ? 'Listening on port ${remote.port}, on every address'
                        : 'Listening on ${remote.listen}, port ${remote.port}',
                    style: TextStyles.body.copyWith(color: palette.textMuted),
                  ),
          ),
          SettingRow(
            label: 'Port',
            child: Row(
              children: [
                SizedBox(
                  width: 96,
                  child: TextField(
                    controller: _port,
                    onSubmitted: (_) => _savePort(),
                  ),
                ),
                const SizedBox(width: Space.m),
                ListenableBuilder(
                  listenable: _port,
                  builder: (context, _) => _port.text.trim() == '${remote.port}'
                      ? const SizedBox.shrink()
                      : Button(label: 'Save', onPressed: _savePort),
                ),
              ],
            ),
          ),
          SettingRow(
            label: 'Listen on',
            child: SizedBox(
              width: 280,
              child: Select(
                value: remote.listen,
                options: [
                  const SelectOption('', 'Every address'),
                  for (final address in listenable)
                    SelectOption(address, address),
                ],
                onChanged: (listen) =>
                    run(() => backend.setRemoteAccess(true, listen: listen)),
              ),
            ),
          ),
        ],
        failureLine(palette),
        if (backend.target case final target? when target.tunnelled)
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
        Text(
          'Paired devices',
          style: TextStyles.caption.copyWith(
            color: palette.textMuted,
            fontWeight: FontWeight.w500,
          ),
        ),
        const SizedBox(height: Space.s),
        if (remote.devices.isEmpty)
          Text(
            'No device is paired yet.',
            style: TextStyles.body.copyWith(color: palette.textFaint),
          ),
        for (final device in remote.devices)
          _DeviceRow(
            device: device,
            thisDevice: device.id == backend.deviceId,
            machine: remote.machine,
            now: now,
          ),
        const SizedBox(height: Space.m),
        Row(
          children: [
            Button(
              label: 'Pair a device…',
              onPressed: remote.on
                  ? () => showModal(
                      context,
                      (context, close) => PairDeviceForm(close: close),
                    )
                  : null,
            ),
            if (!remote.on) ...[
              const SizedBox(width: Space.m),
              Expanded(
                child: Text(
                  'Turn on remote access to pair a device.',
                  style: TextStyles.caption.copyWith(color: palette.textFaint),
                ),
              ),
            ],
          ],
        ),
      ],
    );
  }
}

/// A paired device: its name, whether it's this one, when it was last
/// connected and from where, and Unpair.
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
        title: 'Unpair this device?',
        body:
            'This app disconnects from $machine at once, and can only '
            'connect again with a new pairing code.',
        fields: const [],
        error: null,
        submitLabel: 'Unpair',
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
      if (device.lastAddress.isNotEmpty) device.lastAddress,
      'paired ${pairedOn(device.pairedMs.toInt(), widget.now)}',
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
              Button(label: 'Unpair', onPressed: _unpair),
            ],
          ),
          failureLine(palette),
        ],
      ),
    );
  }
}

/// When something was paired: "today", "yesterday", "12 Nov".
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

/// Pairs a new device: its name and the address it will reach this
/// Factory by, then its pairing code, shown this once.
class PairDeviceForm extends StatefulWidget {
  const PairDeviceForm({super.key, required this.close});

  final VoidCallback close;

  @override
  State<PairDeviceForm> createState() => _PairDeviceFormState();
}

class _PairDeviceFormState extends State<PairDeviceForm> {
  final _name = TextEditingController();
  final _otherHost = TextEditingController();

  /// One of the addresses, or null for the first.
  String? _host;

  /// Another address is being typed in rather than picked.
  var _typing = false;
  var _submitting = false;
  String? _error;
  PairingCode? _code;

  @override
  void dispose() {
    _name.dispose();
    _otherHost.dispose();
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
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: Listenable.merge([store, _name, _otherHost]),
      builder: (context, _) {
        final addresses = store.workspace.remote.addresses;
        final host = addresses.isEmpty || _typing
            ? null
            : _host ?? addresses.first;
        final ready =
            !_submitting &&
            _name.text.trim().isNotEmpty &&
            (host != null || _otherHost.text.trim().isNotEmpty);
        return FormFrame(
          title: 'Pair a device',
          body:
              'Makes a pairing code for one device, such as your laptop. '
              'With it, that device shows this Claktory.',
          error: _error,
          submitLabel: _submitting ? 'Making…' : 'Make pairing code',
          onSubmit: ready ? () => _pair(host) : null,
          onCancel: widget.close,
          fields: [
            Field(
              label: 'Device',
              child: TextField(
                controller: _name,
                placeholder: 'Laptop',
                autofocus: true,
                onSubmitted: (_) {
                  if (ready) _pair(host);
                },
              ),
            ),
            Field(
              label: 'It reaches this Claktory at',
              note:
                  'Addresses on Tailscale (100.x) work from anywhere on '
                  'your tailnet; the others only on this network.',
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (addresses.isNotEmpty)
                    Select<String>(
                      value: host ?? '',
                      options: [
                        for (final address in addresses)
                          SelectOption(address, address),
                        const SelectOption('', 'Another address…'),
                      ],
                      onChanged: (value) => setState(() {
                        _typing = value.isEmpty;
                        _host = value.isEmpty ? null : value;
                      }),
                    ),
                  if (addresses.isEmpty || _typing) ...[
                    if (addresses.isNotEmpty) const SizedBox(height: Space.s),
                    TextField(
                      controller: _otherHost,
                      placeholder: 'A name or address, such as mac-mini.local',
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

  void _pair(String? host) {
    final chosen = _typing || host == null ? _otherHost.text.trim() : host;
    _submitWith(chosen);
  }

  Future<void> _submitWith(String host) async {
    final name = _name.text.trim();
    if (name.isEmpty || _submitting) return;
    setState(() {
      _submitting = true;
      _error = null;
    });
    try {
      final code = await AppScope.of(context).createPairing(name, host);
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
            'Pairing code for $device',
            style: TextStyles.emphasis.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.s),
          Text(
            'On $device, open Claktory, click the Claktory name at the '
            'bottom of the sidebar, and paste this under "Connect to another '
            'Claktory". It\'s shown only this once: it holds $device\'s key.',
            style: TextStyles.body.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.xl),
          Center(child: QrCode(text: widget.code, size: 200)),
          const SizedBox(height: Space.l),
          Container(
            padding: const EdgeInsets.all(Space.m),
            decoration: BoxDecoration(
              color: palette.sidebar,
              borderRadius: BorderRadius.circular(Radii.control),
              border: Border.all(color: palette.hairline),
            ),
            child: Text(
              widget.code,
              style: TextStyles.code.copyWith(color: palette.text),
            ),
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
              Button(label: 'Copy', onPressed: _copy),
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
        label: 'QR code of the pairing code',
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
    final cell = size.width / (modules.size + 2 * quiet);
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
            (x + quiet) * cell,
            (y + quiet) * cell,
            cell + 0.01,
            cell + 0.01,
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
    return ListenableBuilder(
      listenable: Listenable.merge([connections, _code]),
      builder: (context, _) {
        final pasted = _code.text.trim();
        final target = parsePairingCode(pasted);
        final current = connections.current;
        return FormFrame(
          title: 'Claktories',
          body:
              'This app shows one Claktory at a time: this Mac\'s, or one '
              'on another machine it\'s paired with.',
          error: pasted.isNotEmpty && target == null
              ? "That isn't a pairing code. It starts with factory:// and "
                    "comes from that Claktory's Settings → Devices."
              : null,
          submitLabel: 'Connect',
          onSubmit: target == null ? null : () => _show(target),
          onCancel: widget.close,
          fields: [
            _FactoryRow(
              label: 'This Mac',
              detail: 'The Claktory running here',
              showing: current == null,
              onTap: () => _show(null),
            ),
            for (final saved in connections.saved)
              _FactoryRow(
                label: saved.label,
                detail: saved.tunnelled
                    ? 'Through the private tunnel'
                    : '${saved.host}:${saved.port}',
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
              note:
                  'Paste the pairing code made for this device in that '
                  'Claktory\'s Settings → Devices → Pair a device…',
              child: TextField(
                controller: _code,
                placeholder: 'factory://…',
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

/// How this device reaches [machine] through the private tunnel: direct or
/// through a relay, or why it can't.
class TunnelLine extends StatelessWidget {
  const TunnelLine({super.key, required this.status, required this.machine});

  final TunnelStatus status;
  final String machine;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final path = tunnelPathLabel(status.path);
    final failed =
        status.failure != TunnelFailure.none &&
        status.phase != TunnelPhase.connected;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          failed
              ? "Can't reach $machine through the private tunnel"
              : path.isEmpty
              ? 'Reaching $machine through the private tunnel…'
              : 'Reaches $machine through the private tunnel, $path',
          style: TextStyles.body.copyWith(
            color: failed ? palette.danger : palette.textMuted,
          ),
        ),
        if (failed)
          Text(
            [status.error, tunnelAdvice(status, machine)].join(' '),
            style: TextStyles.caption.copyWith(color: palette.textMuted),
          )
        else
          Text(
            status.relayConnected
                ? 'End-to-end encrypted. This device reaches its relay.'
                : 'End-to-end encrypted.',
            style: TextStyles.caption.copyWith(color: palette.textFaint),
          ),
      ],
    );
  }
}

/// Why the app can't reach the Factory it shows through the private
/// tunnel, and what to do: opened from the sidebar's footer.
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
          title: connected ? 'Connected to $machine' : "Can't reach $machine",
          body: connected
              ? 'This Mac reaches $machine through the private tunnel, '
                    '${tunnelPathLabel(now.path)}.'
              : tunnelAdvice(now, machine),
          fields: [
            if (!connected && now.error.isNotEmpty)
              Padding(
                padding: const EdgeInsets.only(bottom: Space.m),
                child: Text(
                  now.error,
                  style: TextStyles.body.copyWith(color: palette.danger),
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
            label: 'Show another Claktory…',
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
