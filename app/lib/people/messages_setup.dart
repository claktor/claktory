import 'dart:async';

import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../devices.dart' show QrCode, pairedOn, thisDeviceKind;
import '../extensions.dart' show dayLabel;
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../mobile/lines.dart' show ActionLine;
import '../mobile/metrics.dart' show TypeScale;
import '../scanner.dart';
import '../scope.dart';
import '../tunnel/api/dm.dart' show DmHandoffSealed;
import '../tunnel/api/dm_transfer.dart';
import '../visible_codes.dart' show proseCodes;
import 'e2ee.dart';
import 'secret_clipboard.dart';

// E5: People's part for end-to-end messages on this device (dms-e2ee §2,
// §5.3; Trish's words): set up messages on the person's first device; on
// another, its QR code for a set-up device to scan, and waiting; once set
// up, the person's devices, adding one by scanning its code and removing
// one. Messages off on a build that can't keep the keys safely, and what
// went wrong when a key can't be read or the account's key was replaced.

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
      // Taken from another device: said once (Fugo M2).
      DmSetup.ready when e2ee.newKeyFrom != null => _Note(
        title: 'Your security key changed',
        'A new security key was made for your account on '
        '${e2ee.newKeyFrom!.isEmpty ? 'another device' : e2ee.newKeyFrom}, '
        "after a lost or stolen device was removed. If that wasn't you, "
        'secure your email and tell your friends.',
        action: Button(label: 'OK', onPressed: e2ee.dismissNewKey),
      ),
      DmSetup.ready when e2ee.finishingKey => const _Note(
        'Finishing your new security key…',
      ),
      DmSetup.ready when e2ee.transfer != null => _Transfer(e2ee: e2ee),
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
          if (text.isNotEmpty)
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
    'Messages with your friends are end-to-end encrypted: only your '
    'devices and theirs can read them, plus any Claktory either of you '
    'gives a chat to, where the agents you allow can read it too. '
    "Claktory's servers can't read what you write, but they do see who you "
    'talk to, when and roughly how much, your devices, your name and '
    'picture, and any messages included in a report. Your friends see the '
    'names of your devices. Until you verify a friend, you\'re trusting '
    'Claktory that their security key is really theirs.\n\n'
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
    unawaited(widget.e2ee.offerThisDevice(platform: thisDeviceKind));
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
    unawaited(widget.e2ee.offerThisDevice(platform: thisDeviceKind));
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
            'On a device where you already use messages, open People, '
            'choose Add a device… under Your devices, and scan this code.',
            style: type.body.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.m),
          Center(
            child: code == null
                ? const SizedBox.square(dimension: 200)
                : QrCode(key: ValueKey(code), text: code, size: 200),
          ),
          if (code == null && widget.e2ee.offerFailed) ...[
            const SizedBox(height: Space.s),
            ActionLine(
              lead: Text(
                "Couldn't make a code.",
                style: type.body.copyWith(color: palette.danger),
              ),
              action: Button(label: 'Try again', onPressed: _make),
            ),
          ],
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

/// A device just added: its history coming over from the device that
/// added it (E8), or what didn't, with Try again while it can.
class _Transfer extends StatelessWidget {
  const _Transfer({required this.e2ee});

  final E2ee e2ee;

  @override
  Widget build(BuildContext context) {
    final transfer = e2ee.transfer!;
    if (transfer.end == null) {
      final total = transfer.chatsTotal;
      return _Note(
        title: 'Bringing your messages over…',
        total == 0
            ? ''
            : '${transfer.chatsDone} of $total ${total == 1 ? 'chat' : 'chats'}',
        action: Button(label: 'Skip', onPressed: e2ee.skipTransfer),
      );
    }
    final actions = Wrap(
      spacing: Space.s,
      runSpacing: Space.s,
      children: [
        if (e2ee.canRetryTransfer)
          Button(label: 'Try again', onPressed: e2ee.retryTransfer),
        Button(label: 'OK', onPressed: e2ee.dismissTransfer),
      ],
    );
    return switch (transfer.end!) {
      DmTransferEnd.cutShort => _Note(
        "Some messages didn't come over.",
        action: actions,
      ),
      _ => _Note(
        'Messages from before '
        '${pairedOn(e2ee.historyFromMs, DateTime.now())} are on your other '
        'devices.',
        action: actions,
      ),
    };
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
  List<DmOwnDevice> _devices = const [];

  /// The person's devices off the list but still in a group here, by
  /// leaf, with the names they claim.
  Map<String, String> _strays = const {};

  /// The account's Claktories by id, to name those still to finish a new
  /// security key; null until read (or when they can't be).
  Map<String, String>? _claktories;

  @override
  void initState() {
    super.initState();
    widget.e2ee.addListener(_read);
    _read();
    unawaited(_readClaktories());
  }

  Future<void> _readClaktories() async {
    try {
      final list = await AppScope.scope(context).connections.account
          .claktories();
      if (mounted) {
        setState(
          () => _claktories = {
            for (final claktory in list) claktory.id: claktory.name,
          },
        );
      }
    } catch (_) {
      // Not signed in, or unreachable: named generically.
    }
  }

  @override
  void dispose() {
    widget.e2ee.removeListener(_read);
    super.dispose();
  }

  void _read() => unawaited(_readNow());

  Future<void> _readNow() async {
    try {
      final devices = await widget.e2ee.devices();
      final strays = await widget.e2ee.notFullyRemoved();
      if (mounted) {
        setState(() {
          _devices = devices;
          _strays = strays;
        });
      }
    } catch (_) {
      // Read again at the next change.
    }
  }

  /// The removal under way, while it isn't done: what it's doing, or
  /// that it stopped, with Try again.
  Widget? _underWay(Palette palette, TypeScale type) {
    final removal = widget.e2ee.removal;
    if (removal == null || removal.remove.isEmpty) return null;
    final names = [
      for (final name in removal.names)
        name.isEmpty ? 'A device' : proseCodes(name),
    ];
    final who = switch (names) {
      [] => 'A device',
      [final one] => one,
      [...final rest, final last] => '${rest.join(', ')} and $last',
    };
    final isnt = names.length > 1 ? "aren't" : "isn't";
    final running = widget.e2ee.removing;
    final words = removal.newKey
        ? running
              ? 'Locking out $who…'
              : '$who $isnt fully locked out yet.'
        : running
        ? 'Removing $who…'
        : '$who $isnt fully removed yet.';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: Space.xs),
      child: ActionLine(
        lead: Text(
          words,
          style: type.body.copyWith(
            color: running ? palette.textMuted : palette.danger,
          ),
        ),
        action: running
            ? const SizedBox.shrink()
            : Button(
                label: 'Try again',
                onPressed: () => unawaited(widget.e2ee.finishRemoval()),
              ),
      ),
    );
  }

  Future<void> _lost(DmOwnDevice device) => showModal(
    context,
    (context, close) => _ChangeKey(
      e2ee: widget.e2ee,
      lost: device,
      devices: _devices,
      close: close,
    ),
  );

  Future<void> _remove(DmOwnDevice device) => showModal(
    context,
    (context, close) => _RemoveDevice(
      e2ee: widget.e2ee,
      leafId: device.leafId,
      name: device.name.isEmpty ? 'this device' : device.name,
      close: close,
    ),
  );

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
                  else ...[
                    Button(
                      label: 'Lost or stolen…',
                      onPressed: () => _lost(device),
                    ),
                    const SizedBox(width: Space.xs),
                    Button(label: 'Remove…', onPressed: () => _remove(device)),
                  ],
                ],
              ),
            ),
          // Off the list, still in a group: a removal that stopped part-way
          // (Abbacchio's R5b), finished from here.
          for (final MapEntry(key: leaf, value: name) in _strays.entries)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: Space.xs),
              child: ActionLine(
                lead: Text.rich(
                  TextSpan(
                    children: [
                      TextSpan(
                        text: name.isEmpty ? 'A device' : proseCodes(name),
                        style: TextStyle(color: palette.text),
                      ),
                      TextSpan(
                        text: '  Not fully removed',
                        style: type.caption.copyWith(color: palette.danger),
                      ),
                    ],
                  ),
                  style: type.body,
                ),
                action: Button(
                  label: 'Finish removing',
                  onPressed: widget.e2ee.removing
                      ? null
                      : () => unawaited(widget.e2ee.finishRemoving(leaf, name)),
                ),
              ),
            ),
          ?_underWay(palette, type),
          // After a new security key: each Claktory until the app connects
          // to it, when it takes the key (Fugo, Ghiaccio).
          for (final id in widget.e2ee.claktoriesToFinish)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: Space.xs),
              child: Text(
                'Connect to ${_claktories?[id] ?? 'your Claktory'} once to '
                'finish your new security key there.',
                style: type.caption.copyWith(color: palette.textMuted),
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
    CameraAccess access;
    try {
      access = await AppScope.scope(context).camera.access();
    } catch (_) {
      // Paste code is the way, as with no camera.
      access = CameraAccess.unavailable;
    }
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
    String? text;
    try {
      text = await secretClipboard.paste();
    } catch (_) {
      // Read as nothing pasted: says so.
    }
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

/// Removing one of the person's devices: out of their list and every
/// group, then signed out of their account (L0, Fugo), so a lost or stolen
/// one can do nothing with what it holds; if the sign-out doesn't go
/// through, it says so and tries again.
class _RemoveDevice extends StatefulWidget {
  const _RemoveDevice({
    required this.e2ee,
    required this.leafId,
    required this.name,
    required this.close,
  });

  final E2ee e2ee;
  final String leafId;
  final String name;
  final VoidCallback close;

  @override
  State<_RemoveDevice> createState() => _RemoveDeviceState();
}

class _RemoveDeviceState extends State<_RemoveDevice> {
  var _busy = false;
  String? _error;

  Future<void> _go() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    final name = widget.name;
    // Kept as under way from here: closed part-way, it carries on at the
    // next launch, and Your devices offers Try again meanwhile.
    final stopped = await widget.e2ee.remove(widget.leafId, name);
    if (stopped == null) {
      widget.close();
      return;
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _error = switch (stopped) {
        DmRemoveStop.failed => "Couldn't remove $name. Try again.",
        // Unreachable leaves it unknown; anything else was a refusal.
        DmRemoveStop.signOutUnreachable =>
          '$name no longer gets new messages, but it may still be signed '
              'in to your Claktory account. Try again.',
        DmRemoveStop.signOutRefused =>
          "$name no longer gets new messages, but it couldn't be signed "
              'out of your Claktory account. Try again.',
      };
    });
  }

  @override
  Widget build(BuildContext context) => FormFrame(
    title: 'Remove ${widget.name}?',
    body:
        'It stops getting new messages and is signed out of your '
        'Claktory account. If you sign in on it again, it starts over as a '
        'new device.\n\n'
        "If it was lost or stolen, also change your email's password, "
        'since your email can sign in new devices. And remove it in '
        "Settings → Devices, so it can't use your Claktory.",
    fields: const [],
    error: _error,
    submitLabel: _error == null ? 'Remove' : 'Try again',
    onSubmit: _busy ? null : _go,
    onCancel: widget.close,
  );
}

/// Lost or stolen (dms-e2ee §2.1): the device is removed and signed out,
/// then the person's messages get a new security key. The person confirms
/// the devices to keep by name and since when; any they untick are
/// removed with it. The lists stay the same across Try again, which
/// carries on where it stopped.
class _ChangeKey extends StatefulWidget {
  const _ChangeKey({
    required this.e2ee,
    required this.lost,
    required this.devices,
    required this.close,
  });

  final E2ee e2ee;
  final DmOwnDevice lost;
  final List<DmOwnDevice> devices;
  final VoidCallback close;

  @override
  State<_ChangeKey> createState() => _ChangeKeyState();
}

class _ChangeKeyState extends State<_ChangeKey> {
  late final List<DmOwnDevice> _others = [
    for (final device in widget.devices)
      if (!device.here && device.leafId != widget.lost.leafId) device,
  ];
  late final Set<String> _keep = {for (final device in _others) device.leafId};

  /// What was confirmed when it started: the same on Try again.
  ({List<String> remove, List<String> keep})? _started;
  var _busy = false;
  String? _error;

  String get _name =>
      widget.lost.name.isEmpty ? 'This device' : widget.lost.name;

  /// [leaf]'s name, kept with the Lock out to say which device it is once
  /// it's off the list.
  String _nameOf(String leaf) =>
      widget.devices.where((each) => each.leafId == leaf).firstOrNull?.name ??
      '';

  Future<void> _go() async {
    final lists = _started ??= (
      remove: [
        widget.lost.leafId,
        for (final device in _others)
          if (!_keep.contains(device.leafId)) device.leafId,
      ],
      keep: [..._keep],
    );
    setState(() {
      _busy = true;
      _error = null;
    });
    final stopped = await widget.e2ee.changeKey(
      remove: lists.remove,
      keep: lists.keep,
      names: [for (final leaf in lists.remove) _nameOf(leaf)],
    );
    if (stopped == null) {
      widget.close();
      return;
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _error = switch (stopped) {
        DmKeyChange.signOutFailed =>
          "$_name no longer gets new messages, but it couldn't be signed "
              "out of your Claktory account, so your security key hasn't "
              'changed yet. Try again.',
        DmKeyChange.failed =>
          "Your new security key isn't finished. Try again to carry on where "
              'it stopped.',
      };
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final now = DateTime.now();
    return FormFrame(
      title: 'Lock out ${widget.lost.name.isEmpty ? 'this device' : _name}?',
      body:
          '$_name is removed and signed out of your Claktory account, and '
          "you get a new security key, so $_name can't read new messages. "
          'Your friends see your security key change.\n\n'
          "Also change your email's password, since your email can sign in "
          "new devices. And remove it in Settings → Devices, so it can't use "
          'your Claktory.',
      fields: [
        if (_others.isNotEmpty) ...[
          Text(
            'Devices to keep',
            style: type.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
          const SizedBox(height: Space.xs),
          for (final device in _others)
            Semantics(
              checked: _keep.contains(device.leafId),
              label: device.name.isEmpty ? 'A device' : device.name,
              excludeSemantics: true,
              child: Pressable(
                onTap: _busy || _started != null
                    ? null
                    : () => setState(() {
                        if (!_keep.remove(device.leafId)) {
                          _keep.add(device.leafId);
                        }
                      }),
                builder: (context, hovered) => Padding(
                  padding: const EdgeInsets.symmetric(vertical: Space.xs),
                  child: Row(
                    children: [
                      CheckMark(on: _keep.contains(device.leafId)),
                      const SizedBox(width: Space.s),
                      Expanded(
                        child: Text(
                          device.name.isEmpty ? 'A device' : device.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: type.body.copyWith(color: palette.text),
                        ),
                      ),
                      if (device.sinceMs > 0)
                        Text(
                          'Since ${_sinceDay(dayLabel(DateTime.fromMillisecondsSinceEpoch(device.sinceMs), now))}',
                          style: type.caption.copyWith(
                            color: palette.textFaint,
                          ),
                        ),
                    ],
                  ),
                ),
              ),
            ),
          const SizedBox(height: Space.xs),
          Text(
            "Untick any you don't recognize, and they're locked out too.",
            style: type.caption.copyWith(color: palette.textFaint),
          ),
        ],
      ],
      error: _error,
      submitLabel: _error == null ? 'Lock out' : 'Try again',
      onSubmit: _busy ? null : _go,
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

/// [dayLabel] mid-line: "today", "yesterday", or the date as it is
/// ("Since today", Trish).
String _sinceDay(String label) => switch (label) {
  'Today' => 'today',
  'Yesterday' => 'yesterday',
  _ => label,
};
