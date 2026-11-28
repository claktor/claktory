// The account's recovery key, made on this device: one key for every
// Claktory on the person's account. The device makes its 20 bytes, shows
// them as the key to save, has one block typed back, and sends only the
// public half of the key pair they make; the Claktory answers the
// statement for the account, which this device checks field by field,
// then signs with its account key once its owner has confirmed it's them
// (Face ID, Touch ID or the passcode), and the Claktory publishes it.
// Nothing here keeps the key: it lives in this form's state until it
// closes.
import 'dart:async';
import 'dart:convert';
import 'dart:math';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'claktory_identity.dart' show claktoryNameInText;
import 'controls.dart';
import 'device_account.dart'
    show
        AccountKeys,
        AccountServiceError,
        AccountServiceUnreachable,
        DeviceAccount,
        RustAccountKeys,
        b64;
import 'device_approval.dart';
import 'forms.dart';
import 'identity.dart' show Space;
import 'recovery_key.dart' show RecoveryKeyBox;
import 'scope.dart';
import 'tunnel/api/account.dart' as rust;
import 'tunnel_client.dart' show RustTunnels;

/// The alphabet a recovery key is written in (RFC 4648's base32).
const _alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';

/// How many bytes a recovery key is.
const recoveryKeyBytes = 20;

/// [bytes] as the key is written to save: 32 characters, in 8 blocks of
/// 4 joined by dashes.
String recoveryKeyText(List<int> bytes) {
  final out = StringBuffer();
  var held = 0, bits = 0, written = 0;
  for (final byte in bytes) {
    held = (held << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      if (written > 0 && written % 4 == 0) out.write('-');
      out.write(_alphabet[(held >> bits) & 31]);
      written++;
      held &= (1 << bits) - 1;
    }
  }
  return out.toString();
}

/// [typed] as the key's characters, read as Claktory reads a typed key:
/// dashes, spaces and case don't matter, and 0, 1 and 8 are the letters O,
/// I and B they look like.
String recoveryKeyTyped(String typed) => typed
    .replaceAll(RegExp(r'[\s-]'), '')
    .toUpperCase()
    .replaceAll('0', 'O')
    .replaceAll('1', 'I')
    .replaceAll('8', 'B');

/// Block [group] (from 1) of the key [text].
String recoveryKeyGroup(String text, int group) => text.split('-')[group - 1];

/// What a device's approval of a recovery key is signed under, before the
/// statement.
const recoveryKeyDeviceLabel = 'claktory recovery key device v1\n';

/// How far a statement's time may be from this device's clock, in seconds:
/// the Claktory takes the signature for as long.
const recoveryStatementWithin = 300;

/// The statement [payload] (base64url) decoded, when it's the one this
/// device asked for: [publicKey] and [deviceKey] its own, [accountId] the
/// account the service has it in, a Claktory among [claktoryIds], a key it
/// replaces that's another, and made within [recoveryStatementWithin] of
/// [now]. Throws [FormatException] naming what doesn't hold.
Uint8List checkRecoveryStatement(
  String payload, {
  required String publicKey,
  required String deviceKey,
  required String accountId,
  required Set<String> claktoryIds,
  required DateTime now,
}) {
  final Uint8List bytes;
  final Object? json;
  try {
    bytes = base64Url.decode(base64Url.normalize(payload));
    json = jsonDecode(utf8.decode(bytes));
  } on FormatException {
    throw const FormatException("isn't a statement");
  }
  if (json is! Map<String, Object?>) {
    throw const FormatException("isn't a statement");
  }
  // Every field is checked below: one more is something else.
  if (json.length != 7) throw const FormatException('has other fields');
  final replaces = json['replaces'];
  final iat = json['iat'];
  if (json['v'] != 1) throw const FormatException('is another version');
  if (json['public_key'] != publicKey) {
    throw const FormatException("isn't for the key made here");
  }
  if (json['device_key'] != deviceKey) {
    throw const FormatException("isn't this device's");
  }
  if (accountId.isEmpty || json['account_id'] != accountId) {
    throw const FormatException("isn't for this device's account");
  }
  if (!claktoryIds.contains(json['claktory_id'])) {
    throw const FormatException("isn't from this Claktory");
  }
  if (replaces is! String ||
      replaces == publicKey ||
      (replaces.isNotEmpty && !_isKey(replaces))) {
    throw const FormatException("doesn't replace another key");
  }
  final seconds = now.millisecondsSinceEpoch ~/ 1000;
  if (iat is! int || (iat - seconds).abs() > recoveryStatementWithin) {
    throw const FormatException("wasn't made just now");
  }
  return bytes;
}

bool _isKey(String text) {
  try {
    return base64Url.decode(base64Url.normalize(text)).length == 32;
  } on FormatException {
    return false;
  }
}

/// [endpoint] (an iroh endpoint id, hex or base32) as lowercase hex; empty
/// when it isn't one.
String endpointHex(String endpoint) {
  final text = endpoint.trim().toLowerCase();
  if (RegExp(r'^[0-9a-f]{64}$').hasMatch(text)) return text;
  if (!RegExp(r'^[a-z2-7]{52}$').hasMatch(text)) return '';
  final out = StringBuffer();
  var held = 0, bits = 0, bytes = 0;
  for (final char in text.toUpperCase().split('')) {
    held = (held << 5) | _alphabet.indexOf(char);
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      if (bytes < 32) {
        out.write(((held >> bits) & 255).toRadixString(16).padLeft(2, '0'));
      }
      bytes++;
      held &= (1 << bits) - 1;
    }
  }
  return out.toString();
}

/// What making the account's key needs of this device's own sign-in to
/// the account.
abstract interface class RecoveryKeyDevice {
  /// This device's account key's public half (base64url): the one its
  /// connection to the Claktory was let in with. StateError when it isn't
  /// signed in.
  Future<String> deviceKey();

  /// The account the service has this device in.
  Future<String> accountId();

  /// The account's Claktories that may be the one this app shows: the one
  /// at [endpoint] (lowercase hex) when it's known, else every one.
  Future<Set<String>> claktoryIds(String endpoint);

  /// [message] signed with this device's account key.
  Future<List<int>> sign(List<int> message);

  /// The public half of the key pair a recovery key's [bytes] make
  /// (base64url).
  Future<String> recoveryPublicKey(List<int> bytes);
}

/// [RecoveryKeyDevice] through this device's sign-in to the account.
class SignedInRecoveryKeyDevice implements RecoveryKeyDevice {
  const SignedInRecoveryKeyDevice(
    this.account, {
    this.keys = const RustAccountKeys(),
  });

  final DeviceAccount account;
  final AccountKeys keys;

  String get _key {
    final key = account.key;
    if (!account.signedIn || key.isEmpty) {
      throw StateError("This device isn't signed in to the account.");
    }
    return key;
  }

  @override
  Future<String> deviceKey() async => b64(await keys.publicKey(_key));

  @override
  Future<String> accountId() async {
    final me = await account.whoAmI();
    if (me == null) {
      throw StateError("This device isn't signed in to the account.");
    }
    return me.accountId;
  }

  @override
  Future<Set<String>> claktoryIds(String endpoint) async => {
    for (final claktory in await account.claktories())
      if (endpoint.isEmpty || endpointHex(claktory.endpointId) == endpoint)
        claktory.id,
  };

  @override
  Future<List<int>> sign(List<int> message) => keys.sign(_key, message);

  @override
  Future<String> recoveryPublicKey(List<int> bytes) async {
    await RustTunnels.load();
    return b64(rust.recoveryPublicKey(key: bytes));
  }
}

/// The words the owner's prompt gives the reason in ([ownerPrompt]).
const makeRecoveryKeyReason =
    'make a new recovery key for your Claktory account';

/// Makes [bytes] the account's recovery key through [backend], the
/// Claktory this app shows (at [endpoint], lowercase hex, or empty when
/// unknown): the statement is checked before the owner is asked, and
/// signed only once they're confirmed. Throws what [checkRecoveryStatement],
/// [DeviceApprover.confirmOwner], [RecoveryKeyDevice] and [backend] throw;
/// nothing is published unless it returns.
Future<void> makeAccountRecoveryKey({
  required Backend backend,
  required RecoveryKeyDevice device,
  required List<int> bytes,
  required String endpoint,
  DateTime Function() now = DateTime.now,
}) async {
  final publicKey = await device.recoveryPublicKey(bytes);
  final deviceKey = await device.deviceKey();
  final payload = await backend.newAccountRecoveryKey(
    publicKey: publicKey,
    deviceKey: deviceKey,
  );
  final statement = checkRecoveryStatement(
    payload,
    publicKey: publicKey,
    deviceKey: deviceKey,
    accountId: await device.accountId(),
    claktoryIds: await device.claktoryIds(endpoint),
    now: now(),
  );
  await deviceApprover.confirmOwner(reason: makeRecoveryKeyReason);
  final signature = await device.sign([
    ...utf8.encode(recoveryKeyDeviceLabel),
    ...statement,
  ]);
  await backend.publishRecoveryKey(
    deviceKey: deviceKey,
    deviceSig: b64(signature),
  );
}

/// Make a new recovery key…: the key made here and shown, a block of it
/// typed back, then made the account's.
class MakeRecoveryKeyForm extends StatefulWidget {
  const MakeRecoveryKeyForm({
    super.key,
    required this.close,
    this.device,
    this.random,
  });

  final VoidCallback close;

  /// This device's sign-in to the account when null.
  final RecoveryKeyDevice? device;

  /// Where the key's bytes come from; a secure source when null.
  final Random? random;

  @override
  State<MakeRecoveryKeyForm> createState() => _MakeRecoveryKeyFormState();
}

class _MakeRecoveryKeyFormState extends State<MakeRecoveryKeyForm> {
  late final Random _random = widget.random ?? Random.secure();
  late final Uint8List _bytes = Uint8List.fromList([
    for (var i = 0; i < recoveryKeyBytes; i++) _random.nextInt(256),
  ]);
  late final String _text = recoveryKeyText(_bytes);

  /// The block to type back, once the person said they saved the key.
  int? _group;
  final _typed = TextEditingController();
  var _busy = false;
  var _copied = false;
  String? _error;

  @override
  void dispose() {
    _typed.dispose();
    _bytes.fillRange(0, _bytes.length, 0);
    super.dispose();
  }

  void _ask() => setState(() {
    _group = 1 + _random.nextInt(_text.split('-').length);
    _typed.clear();
    _error = null;
  });

  Future<void> _make() async {
    final group = _group;
    if (group == null || _busy) return;
    if (recoveryKeyTyped(_typed.text) != recoveryKeyGroup(_text, group)) {
      setState(
        () => _error =
            "That isn't block $group of the key. Check what you saved, or go "
            'back to see the key again.',
      );
      return;
    }
    final backend = AppScope.of(context);
    final device =
        widget.device ??
        SignedInRecoveryKeyDevice(AppScope.scope(context).connections.account);
    setState(() {
      _busy = true;
      _error = null;
    });
    String? error;
    try {
      await makeAccountRecoveryKey(
        backend: backend,
        device: device,
        bytes: _bytes,
        endpoint: endpointHex(backend.target?.endpointId ?? ''),
      );
      if (mounted) widget.close();
      return;
    } on OwnerNotAuthenticated catch (refused) {
      error = switch (refused.reason) {
        OwnerAuthFailure.cancelled => null,
        OwnerAuthFailure.failed =>
          "This device couldn't confirm it's you, so the key wasn't made.",
        OwnerAuthFailure.unavailable =>
          'To make a recovery key here, set up a passcode, Touch ID or a '
              'login password on this device first.',
      };
    } on FormatException {
      error =
          'Claktory asked this device to approve something other than your '
          "new key, so it didn't, and the key wasn't made.";
    } on UnimplementedError {
      error =
          "Claktory on Android can't make recovery keys yet. Make it on your "
          'iPhone, iPad or Mac.';
    } on StateError {
      error =
          'To make a recovery key, sign in to your Claktory account on this '
          'device first.';
    } on RequestFailure catch (failure) {
      error = failure.message;
    } on AccountServiceError {
      error =
          "Claktory couldn't check your account, so the key wasn't made. Try "
          'again.';
    } on AccountServiceUnreachable {
      error =
          "Can't reach your account right now, so the key wasn't made. Try "
          'again.';
    }
    if (mounted) {
      setState(() {
        _busy = false;
        _error = error;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final cannot = backend.cannotApprove.value;
    if (cannot.isNotEmpty) {
      return FormFrame(
        title: 'Make a new recovery key',
        // Connected with a code: the backend's words are about approving.
        body: backend.target == null
            ? cannot
            : 'This device connected to ${claktoryNameInText(backend)} with a '
                  'code, not with your Claktory account. To make a recovery '
                  'key here, sign in to your account on this device and '
                  'connect again.',
        fields: const [],
        error: null,
        submitLabel: 'Close',
        onSubmit: widget.close,
        onCancel: widget.close,
      );
    }
    final group = _group;
    if (group != null) return _checking(group);
    return FormFrame(
      title: 'New recovery key for your account',
      body:
          "If you ever can't open your secrets, this key opens them on every "
          "Claktory on your account. Once it's made, the keys you saved "
          'before stop working, except on copies of your secrets made '
          'earlier.\n\n'
          'Save it in a password manager, such as Passwords or 1Password, or '
          "on paper kept somewhere safe. Claktory doesn't keep a copy, so "
          "once you close this it can't be shown again. Never share it: "
          'anyone with it can open your secrets.',
      fields: [
        RecoveryKeyBox(_text),
        const SizedBox(height: Space.s),
        Align(
          alignment: AlignmentDirectional.centerStart,
          child: Button(
            label: _copied ? 'Copied' : 'Copy',
            onPressed: () {
              Clipboard.setData(ClipboardData(text: _text));
              setState(() => _copied = true);
            },
          ),
        ),
        const SizedBox(height: Space.l),
      ],
      error: _error,
      dirty: true,
      submitLabel: "I've saved it",
      onSubmit: _ask,
      onCancel: widget.close,
    );
  }

  Widget _checking(int group) => FormFrame(
    title: 'Check the recovery key',
    body:
        'Type block $group from the key you saved, counting the blocks of '
        'four from the left.',
    fields: [
      Field(
        label: 'Block $group',
        child: TextField(
          controller: _typed,
          autofocus: true,
          autocorrect: false,
          textCapitalization: TextCapitalization.characters,
          onSubmitted: (_) => _make(),
        ),
      ),
    ],
    error: _error,
    dirty: true,
    submitLabel: _busy ? 'Making…' : 'Make key',
    onSubmit: _busy ? null : _make,
    cancelLabel: 'Back',
    onCancel: _busy
        ? () {}
        : () => setState(() {
            _group = null;
            _error = null;
          }),
  );
}
