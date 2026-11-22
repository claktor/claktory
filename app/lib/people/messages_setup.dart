import 'dart:async';

import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../devices.dart' show QrCode, thisDeviceKind;
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../mobile/metrics.dart' show TypeScale;
import '../scanner.dart';
import '../scope.dart';
import '../tunnel/api/dm.dart' show DmHandoffSealed;
import 'e2ee.dart';
import 'secret_clipboard.dart';

// E5: People's part for end-to-end messages on this device (dms-e2ee §2,
// §5.3; Trish's words): set up messages on the person's first device; on
// another, its QR code for a set-up device to scan, and waiting; once set
// up, the person's devices, adding one by scanning its code and removing
// one. Messages off on a build that can't keep the keys safely, and what
// went wrong when a key can't be read or the account's key was replaced.

/// The identity for the iroh endpoint a new device takes history on (E8):
/// until E8, none yet.
List<int> Function() historyEndpoint = () => List.filled(32, 0);

/// Where a new device's code is copied and pasted; tests give their own.
SecretClipboard secretClipboard = const SecretClipboard();

/// Messages on this device, in People: what the person needs to do, at
/// the top; or, with [devices], their devices once set up, further down.
class MessagesSetup extends StatelessWidget {
  const MessagesSetup({super.key, required this.e2ee, this.devices = false});

  final E2ee e2ee;

  /// Only Your devices, and only once set up.
  final bool devices;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: e2ee,
    builder: (context, _) => switch (e2ee.setup) {
      DmSetup.ready when devices => _Devices(e2ee: e2ee),
      _ when devices => const SizedBox.shrink(),
      DmSetup.starting ||
      DmSetup.signedOut ||
      DmSetup.ready => const SizedBox.shrink(),
      DmSetup.unavailable => _Note(
        "Messages can't be used in this version of Claktory, because it "
        "can't keep your security key safe on this Mac. Use the Claktory "
        'app, or another of your devices.',
      ),
      DmSetup.firstDevice => _SetUp(e2ee: e2ee),
      DmSetup.addThisDevice => _AddThis(e2ee: e2ee),
      DmSetup.failed => _Failed(e2ee: e2ee),
    },
  );
}

class _Note extends StatelessWidget {
  const _Note(this.text, {this.title, this.danger = false, this.action});

  final String? title;
  final String text;
  final bool danger;
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (title case final title?) ...[
            Text(
              title,
              style: type.emphasis.copyWith(
                color: danger ? palette.danger : palette.text,
              ),
            ),
            const SizedBox(height: Space.xs),
          ],
          Text(text, style: type.body.copyWith(color: palette.textMuted)),
          if (action case final action?) ...[
            const SizedBox(height: Space.s),
            action,
          ],
        ],
      ),
    );
  }
}

class _SetUp extends StatefulWidget {
  const _SetUp({required this.e2ee});

  final E2ee e2ee;

  @override
  State<_SetUp> createState() => _SetUpState();
}

class _SetUpState extends State<_SetUp> {
  var _busy = false;

  Future<void> _setUp() async {
    setState(() => _busy = true);
    await widget.e2ee.setUpFirst();
    if (mounted) setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) => _Note(
    title: 'Set up messages',
    'Your messages are locked with a security key that stays on your '
    'devices. Set up messages on more than one device: if you lose them '
    'all, your messages are gone, and your friends see your security key '
    'change.',
    action: Button(
      label: 'Set up',
      primary: true,
      onPressed: _busy ? null : _setUp,
    ),
  );
}

/// This device isn't added yet: its QR code, for one of the person's
/// set-up devices to scan.
class _AddThis extends StatefulWidget {
  const _AddThis({required this.e2ee});

  final E2ee e2ee;

  @override
  State<_AddThis> createState() => _AddThisState();
}

class _AddThisState extends State<_AddThis> {
  /// Whether this device can copy its code safely; until then, and where
  /// it can't, no Copy code.
  var _canCopy = false;

  /// The code copied, and the token to clear it with.
  String? _copied;
  int? _token;

  /// A code works for 30 minutes (the service keeps a hand-off no longer):
  /// then a new one.
  Timer? _renew;

  @override
  void initState() {
    super.initState();
    _offer();
    unawaited(
      secretClipboard.available.then((can) {
        if (mounted) setState(() => _canCopy = can);
      }),
    );
  }

  @override
  void dispose() {
    _renew?.cancel();
    // Added, or gone from here: the code is no use to anyone now.
    _forget();
    super.dispose();
  }

  void _forget() {
    if (_token case final token?) unawaited(secretClipboard.clear(token));
    _token = null;
    _copied = null;
  }

  Future<void> _copy(String code) async {
    _forget();
    final token = await secretClipboard.copy(code);
    if (!mounted) {
      if (token != null) unawaited(secretClipboard.clear(token));
      return;
    }
    setState(() {
      _token = token;
      _copied = token == null ? null : code;
    });
  }

  @override
  void didUpdateWidget(_AddThis old) {
    super.didUpdateWidget(old);
    _offer();
  }

  void _offer() {
    // A spent code copied is cleared, and Copy code is back.
    if (_copied != null && _copied != widget.e2ee.offer) _forget();
    // A new one when the last was spent (a hand-off that didn't open) or
    // is too old.
    if (_renew != null && widget.e2ee.offer != null) return;
    final age = widget.e2ee.offerAge;
    const life = Duration(seconds: SecretClipboard.seconds);
    if (widget.e2ee.offer != null && age < life) {
      _renewIn(life - age);
      return;
    }
    _renewIn(life);
    unawaited(
      widget.e2ee.offerThisDevice(
        platform: thisDeviceKind,
        endpointId: historyEndpoint(),
      ),
    );
  }

  void _renewIn(Duration after) {
    _renew?.cancel();
    _renew = Timer(after, () {
      _renew = null;
      _forget();
      _make();
    });
  }

  /// A new code, and the old one forgotten.
  void _make() {
    if (!mounted) return;
    _renewIn(const Duration(seconds: SecretClipboard.seconds));
    unawaited(
      widget.e2ee.offerThisDevice(
        platform: thisDeviceKind,
        endpointId: historyEndpoint(),
      ),
    );
    setState(() {});
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final code = widget.e2ee.offer;
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Add this $thisDeviceKind to your messages',
            style: type.emphasis.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.xs),
          Text(
            'On a device where you already use messages, open People → '
            'Devices, choose Add a device…, and scan this code.',
            style: type.body.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.m),
          Center(
            child: code == null
                ? const SizedBox.square(dimension: 200)
                : QrCode(key: ValueKey(code), text: code, size: 200),
          ),
          const SizedBox(height: Space.s),
          Text(
            'Waiting for your other device…',
            style: type.caption.copyWith(color: palette.textFaint),
          ),
          if (_canCopy && code != null) ...[
            const SizedBox(height: Space.m),
            Button(
              label: _copied == code ? 'Copied' : 'Copy code',
              onPressed: () => _copy(code),
            ),
            const SizedBox(height: Space.s),
            _Digits(widget.e2ee.offerDigits ?? ''),
            const SizedBox(height: Space.xs),
            Text(
              'If you paste this code instead of scanning it, check that your '
              'other device shows these same numbers. The code works once, '
              'for 30 minutes.',
              style: type.caption.copyWith(color: palette.textFaint),
            ),
          ],
        ],
      ),
    );
  }
}

class _Failed extends StatelessWidget {
  const _Failed({required this.e2ee});

  final E2ee e2ee;

  Future<void> _startOver(BuildContext context) async {
    var sure = false;
    await showModal(
      context,
      (context, close) => FormFrame(
        title: 'Start over on this device?',
        body:
            'Messages kept only on this device are lost. Your other devices '
            'keep theirs.',
        fields: const [],
        error: null,
        submitLabel: 'Start over',
        onSubmit: () {
          sure = true;
          close();
        },
        onCancel: close,
      ),
    );
    if (sure) await e2ee.startOver();
  }

  @override
  Widget build(BuildContext context) {
    final startOver = Builder(
      builder: (context) =>
          Button(label: 'Start over…', onPressed: () => _startOver(context)),
    );
    return switch (e2ee.failure) {
      DmFailure.identityReplaced => _Note(
        title: 'Your security key changed on another device',
        danger: true,
        'A new security key was made for your account on another device, so '
        'this device no longer gets new messages. If that was you, add this '
        "device again from there. If it wasn't, someone may be using your "
        'email: secure it, and tell your friends.',
        action: startOver,
      ),
      DmFailure.unreadableKey => _Note(
        "This device can't read its security key, so its messages can't be "
        'opened.',
        action: startOver,
      ),
      _ => _Note(
        "Messages couldn't be set up on this device.",
        action: Button(label: 'Try again', onPressed: e2ee.start),
      ),
    };
  }
}

/// The person's devices, this one first among them, with Add a device….
class _Devices extends StatefulWidget {
  const _Devices({required this.e2ee});

  final E2ee e2ee;

  @override
  State<_Devices> createState() => _DevicesState();
}

class _DevicesState extends State<_Devices> {
  List<({String leafId, String name, bool here})> _devices = const [];

  @override
  void initState() {
    super.initState();
    widget.e2ee.addListener(_read);
    _read();
  }

  @override
  void dispose() {
    widget.e2ee.removeListener(_read);
    super.dispose();
  }

  void _read() => unawaited(
    widget.e2ee.devices().then((devices) {
      if (mounted) setState(() => _devices = devices);
    }),
  );

  Future<void> _remove(({String leafId, String name, bool here}) device) async {
    var sure = false;
    final name = device.name.isEmpty ? 'this device' : device.name;
    await showModal(
      context,
      (context, close) => FormFrame(
        title: 'Remove $name from your messages?',
        body: 'It stops getting new messages.',
        fields: const [],
        error: null,
        submitLabel: 'Remove',
        onSubmit: () {
          sure = true;
          close();
        },
        onCancel: close,
      ),
    );
    if (sure) await widget.e2ee.removeDevice(device.leafId);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final devices = [
      ..._devices.where((device) => device.here),
      ..._devices.where((device) => !device.here),
    ];
    return Field(
      label: 'Your devices',
      note: 'These can read your messages.',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final device in devices)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: Space.xs),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      device.name.isEmpty ? 'A device' : device.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: type.body.copyWith(color: palette.text),
                    ),
                  ),
                  if (device.here)
                    Text(
                      'This device',
                      style: type.caption.copyWith(color: palette.textFaint),
                    )
                  else
                    Button(label: 'Remove…', onPressed: () => _remove(device)),
                ],
              ),
            ),
          const SizedBox(height: Space.s),
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(
              label: 'Add a device…',
              onPressed: () => showAddDevice(context, widget.e2ee),
            ),
          ),
        ],
      ),
    );
  }
}

/// Scans a new device's code, asks the person, and adds it.
Future<void> showAddDevice(BuildContext context, E2ee e2ee) => showModal(
  context,
  (context, close) => _AddDevice(e2ee: e2ee, close: close),
);

class _AddDevice extends StatefulWidget {
  const _AddDevice({required this.e2ee, required this.close});

  final E2ee e2ee;
  final VoidCallback close;

  @override
  State<_AddDevice> createState() => _AddDeviceState();
}

class _AddDeviceState extends State<_AddDevice> {
  CameraAccess? _access;
  DmHandoffSealed? _found;
  String? _error;
  var _busy = false;

  /// The code was pasted, not scanned: Add waits until the person says
  /// the digits match the new device's ([_match]).
  var _pasted = false;
  var _match = false;

  @override
  void initState() {
    super.initState();
    unawaited(_ask());
  }

  Future<void> _ask() async {
    final access = await AppScope.scope(context).camera.access();
    if (mounted) setState(() => _access = access);
  }

  Future<void> _scanned(String code, {bool pasted = false}) async {
    if (_found != null || _busy) return;
    _busy = true;
    try {
      // The same reading for a pasted code as for a scanned one.
      final found = await widget.e2ee.scanned(code);
      if (mounted) {
        setState(() {
          _found = found;
          _pasted = pasted;
          _match = false;
          _error = null;
        });
      }
    } catch (_) {
      if (mounted) {
        setState(
          () => _error = pasted
              ? "That isn't a code from a new device. Copy it again from the "
                    "new device's screen."
              : "That isn't a code for adding a device.",
        );
      }
    }
    _busy = false;
  }

  Future<void> _paste() async {
    final text = await secretClipboard.paste();
    await _scanned(text ?? '', pasted: true);
  }

  Future<void> _add() async {
    final found = _found;
    if (found == null) return;
    setState(() => _busy = true);
    try {
      await widget.e2ee.addDevice(found);
      widget.close();
    } catch (_) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = "Couldn't add it. Try again.";
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final found = _found;
    if (found != null) {
      final name = found.name.isEmpty ? 'This device' : found.name;
      final platform = found.platform.isEmpty ? '' : ' (${found.platform})';
      return FormFrame(
        title: 'Add this device to your messages?',
        body:
            '$name$platform will be able to read all your messages. Add it '
            "only if it's yours.",
        fields: [
          if (_pasted) ...[
            Text(
              'Check these numbers match the ones on your new device.',
              style: type.body.copyWith(color: palette.text),
            ),
            const SizedBox(height: Space.s),
            _Digits(found.comparison),
            const SizedBox(height: Space.s),
            Semantics(
              checked: _match,
              label: 'The numbers match',
              excludeSemantics: true,
              child: Pressable(
                onTap: () => setState(() => _match = !_match),
                builder: (context, hovered) => Row(
                  children: [
                    CheckMark(on: _match),
                    const SizedBox(width: Space.s),
                    Text(
                      'The numbers match',
                      style: type.body.copyWith(color: palette.text),
                    ),
                  ],
                ),
              ),
            ),
            const SizedBox(height: Space.xs),
            Text(
              "If they don't match, don't add it. Copy the code on your new "
              'device again.',
              style: type.caption.copyWith(color: palette.textFaint),
            ),
          ],
        ],
        error: _error,
        submitLabel: 'Add',
        onSubmit: _busy || (_pasted && !_match) ? null : _add,
        onCancel: widget.close,
      );
    }
    final camera = AppScope.scope(context).camera;
    return FormFrame(
      title: 'Scan the code on your new device',
      fields: [
        if (_access == CameraAccess.granted)
          AspectRatio(
            aspectRatio: 1,
            child: Container(
              clipBehavior: Clip.antiAlias,
              decoration: BoxDecoration(
                color: const Color(0xFF000000),
                borderRadius: BorderRadius.circular(Radii.panel),
                border: Border.all(color: palette.hairline),
              ),
              child: camera.preview((code) => unawaited(_scanned(code))),
            ),
          )
        else if (_access == CameraAccess.undetermined)
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(
              label: 'Allow camera',
              primary: true,
              onPressed: () async {
                await camera.request();
                await _ask();
              },
            ),
          )
        else if (_access != null)
          Text(
            "This $thisDeviceKind can't scan. Copy the code on your new "
            'device and choose Paste code instead.',
            style: type.body.copyWith(color: palette.textMuted),
          ),
        const SizedBox(height: Space.s),
        Align(
          alignment: AlignmentDirectional.centerStart,
          child: Button(label: 'Paste code instead', onPressed: _paste),
        ),
      ],
      error: _error,
      submitLabel: 'Done',
      onSubmit: null,
      onCancel: widget.close,
    );
  }
}

/// The digits two devices compare for a pasted code, in four groups.
class _Digits extends StatelessWidget {
  const _Digits(this.digits);

  final String digits;

  @override
  Widget build(BuildContext context) => Text(
    digits,
    style: TypeScale.of(context).emphasis.copyWith(
      color: Palette.of(context).text,
      fontFeatures: const [FontFeature.tabularFigures()],
      letterSpacing: 0.5,
    ),
  );
}
