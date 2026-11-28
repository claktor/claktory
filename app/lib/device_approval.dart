// Prosciutto's (Mobile): the device's own approvals, signed with its
// account key only after its owner authenticates (Fugo's server-authorize.md
// item 5): Face ID, Touch ID or the passcode on iPhone and iPad
// (LocalAuthentication's deviceOwnerAuthentication, in AppDelegate.swift's
// OwnerAuth); the Mac's and Android's follow through their owners, and say
// they can't approve yet until then. Never from a notification, a URL,
// launch arguments or the Claktory: only a person's deliberate act in the
// app (Settings → Move's approval, move_approval.dart; a join's card in
// account_join.dart).
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'device_account.dart'
    show AccountKeys, DeviceAccount, RustAccountKeys, b64;
import 'move_approval.dart' show endpointFingerprint;

/// Why the owner wasn't authenticated, and so nothing was signed.
enum OwnerAuthFailure {
  /// The person cancelled the system's prompt.
  cancelled,

  /// The system couldn't confirm it's them.
  failed,

  /// The device has no passcode, biometrics or login password to ask for:
  /// it never approves.
  unavailable,
}

/// The owner wasn't authenticated: nothing was signed.
class OwnerNotAuthenticated implements Exception {
  const OwnerNotAuthenticated(this.reason);

  final OwnerAuthFailure reason;

  @override
  String toString() => 'OwnerNotAuthenticated(${reason.name})';
}

/// The longest a move approval may work, from its iat to its exp: a
/// request's own life (API.md, "for 10 minutes"). The source refuses longer
/// ones; the signer refuses to make them.
const moveApprovalMaxLife = 600;

/// What a device's move approval is signed under, before its payload
/// (claktory-account's MOVE_APPROVAL_LABEL).
const moveApprovalLabel = 'claktory move approval v1\n';

abstract interface class DeviceApprover {
  /// Has the device's owner authenticate first ([reason] is the prompt's
  /// words in lower case, such as "approve moving Studio to the computer
  /// with code C77E F7E5 EBF2 A13B": see [ownerPrompt]), then
  /// signs a move approval with [account]'s own key exactly as
  /// claktory-account's protocol::sign_move_approval does: payload
  /// {"v":1,"kind":"move","request_id","account_id","source_claktory_id",
  /// "endpoint","fingerprint","device_key","iat","exp"} in that order,
  /// device_key the account key's public half (base64url, no padding), and
  /// answers b64(payload) "." b64(signature over "claktory move approval
  /// v1\n" ‖ payload). [fingerprint] is the code the card showed, which
  /// must be the one worked out from [endpoint] here. [iat] and [exp] are
  /// unix seconds on the service's clock; [exp] is the request's end,
  /// later than [iat] by no more than [moveApprovalMaxLife].
  ///
  /// Throws [OwnerNotAuthenticated] with nothing signed; ArgumentError,
  /// before asking the owner, for a fingerprint, field or time that doesn't
  /// hold; StateError for a device not signed in; UnimplementedError where
  /// this device can't ask its owner yet.
  Future<String> approveMove({
    required DeviceAccount account,
    required String requestId,
    required String accountId,
    required String sourceClaktoryId,
    required String endpoint,
    required String fingerprint,
    required int iat,
    required int exp,
    required String reason,
  });

  /// Only the owner's check, signing nothing: before a join's approval
  /// (account_join.dart's card), whose device-signed request already
  /// carries its fingerprint. [reason] is the prompt's words. Throws as
  /// [approveMove] does: [OwnerNotAuthenticated], ArgumentError for an
  /// empty [reason] (never asked), UnimplementedError where this device
  /// can't ask yet.
  Future<void> confirmOwner({required String reason});
}

/// Asks the owner through the system (factory/device's authenticateOwner)
/// and returns once they're confirmed; throws [OwnerNotAuthenticated]
/// otherwise, and UnimplementedError where the platform can't ask yet.
Future<void> authenticateOwner(String reason) async {
  final String? answer;
  try {
    answer = await const MethodChannel('factory/device').invokeMethod<String>(
      'authenticateOwner',
      {'reason': ownerPrompt(reason)},
    );
  } on MissingPluginException {
    throw UnimplementedError("This device can't ask for its owner yet.");
  } on PlatformException {
    throw const OwnerNotAuthenticated(OwnerAuthFailure.failed);
  }
  switch (answer) {
    case 'ok':
      return;
    case 'cancelled':
      throw const OwnerNotAuthenticated(OwnerAuthFailure.cancelled);
    case 'unavailable':
      throw const OwnerNotAuthenticated(OwnerAuthFailure.unavailable);
    default:
      throw const OwnerNotAuthenticated(OwnerAuthFailure.failed);
  }
}

/// [reason] as the system's prompt shows it: as written on a Mac, where
/// macOS puts "Claktory is trying to" in front of it ("approve moving
/// Studio to…"), with a capital first letter elsewhere.
/// Callers write it in lower case.
String ownerPrompt(String reason) =>
    defaultTargetPlatform == TargetPlatform.macOS || reason.isEmpty
    ? reason
    : reason[0].toUpperCase() + reason.substring(1);

/// The app's [DeviceApprover]: the owner first, then the device's key.
class OwnerApprover implements DeviceApprover {
  const OwnerApprover({
    this.keys = const RustAccountKeys(),
    this.authenticate = authenticateOwner,
  });

  final AccountKeys keys;
  final Future<void> Function(String reason) authenticate;

  @override
  Future<void> confirmOwner({required String reason}) async {
    if (reason.isEmpty) throw ArgumentError.value(reason, 'reason', 'is empty');
    await authenticate(reason);
  }

  @override
  Future<String> approveMove({
    required DeviceAccount account,
    required String requestId,
    required String accountId,
    required String sourceClaktoryId,
    required String endpoint,
    required String fingerprint,
    required int iat,
    required int exp,
    required String reason,
  }) async {
    // Everything that can be wrong with the card, before the owner is
    // asked: a bad card never puts up a prompt.
    if (!(iat < exp && exp - iat <= moveApprovalMaxLife)) {
      throw ArgumentError(
        'A move approval works for up to $moveApprovalMaxLife s after '
        'it was made: iat $iat, exp $exp.',
      );
    }
    // The code signed is the one worked out here from the computer's key,
    // never a service's word for it.
    final code = endpointFingerprint(endpoint);
    if (code == null || code != fingerprint.toLowerCase()) {
      throw ArgumentError.value(
        fingerprint,
        'fingerprint',
        "isn't the code of the endpoint it approves",
      );
    }
    for (final (name, value) in [
      ('requestId', requestId),
      ('accountId', accountId),
      ('sourceClaktoryId', sourceClaktoryId),
      ('reason', reason),
    ]) {
      if (value.isEmpty) throw ArgumentError.value(value, name, 'is empty');
    }
    // This device's own key, read here, never handed in.
    final key = account.key;
    if (!account.signedIn || key.isEmpty) {
      throw StateError("This device isn't signed in to the account.");
    }
    await authenticate(reason);
    final payload = moveApprovalPayload(
      requestId: requestId,
      accountId: accountId,
      sourceClaktoryId: sourceClaktoryId,
      endpoint: endpoint,
      fingerprint: code,
      deviceKey: b64(await keys.publicKey(key)),
      iat: iat,
      exp: exp,
    );
    final signature = await keys.sign(key, [
      ...utf8.encode(moveApprovalLabel),
      ...payload,
    ]);
    return '${b64(payload)}.${b64(signature)}';
  }
}

/// A move approval's payload, byte for byte as serde_json writes
/// claktory-account's MoveApproval: its fields in the struct's order, with
/// no spaces.
List<int> moveApprovalPayload({
  required String requestId,
  required String accountId,
  required String sourceClaktoryId,
  required String endpoint,
  required String fingerprint,
  required String deviceKey,
  required int iat,
  required int exp,
}) => utf8.encode(
  jsonEncode({
    'v': 1,
    'kind': 'move',
    'request_id': requestId,
    'account_id': accountId,
    'source_claktory_id': sourceClaktoryId,
    'endpoint': endpoint,
    'fingerprint': fingerprint,
    'device_key': deviceKey,
    'iat': iat,
    'exp': exp,
  }),
);

/// The app's; tests set their own.
DeviceApprover deviceApprover = const OwnerApprover();
