import 'dart:math';

import 'package:flutter/widgets.dart';

import '../device_account.dart';
import '../scope.dart';

// U3a: account-level apps (People, and later the person's own apps) talk
// to the account service as this device, signed in to the person's
// Claktory account, above whichever Claktory is shown: they work with
// every Claktory off. Nothing new to sign in to: it's device_account.dart's
// sign-in and its key, and nothing of what they answer is kept here.

/// Why an account-level app's request didn't work.
enum UserAppProblem {
  /// This device isn't signed in to a Claktory account (or was signed out
  /// from elsewhere).
  notSignedIn,

  /// Only the person's own devices may ask (a Claktory or agent got here).
  deviceOnly,

  /// Another device changed it first: read it again.
  stale,

  /// Too many tries for now; the message says how long to wait.
  rateLimited,

  /// The service, or the app behind it, is down for now.
  unavailable,

  /// The service can't be reached from here.
  unreachable,

  /// It isn't there, or isn't this person's.
  notFound,

  /// It isn't open anymore (a request cancelled, declined or expired).
  gone,

  /// The other side got there first (a request accepted before it could
  /// be cancelled).
  accepted,

  /// The service refused what was sent.
  invalid,

  /// The person's messages take all the space they may keep: deleting
  /// some makes room (413 storage_full).
  storageFull,

  /// End to end: the group moved on to another epoch first (409 epoch,
  /// `details['epoch']`): catch up and make it again.
  epoch,

  /// End to end: the group already exists (409 exists).
  exists,

  /// End to end: this device is past the group's mailbox (410
  /// too_far_behind): it joins again.
  tooFarBehind,

  /// End to end: the account has no keys yet (409 not_set_up).
  notSetUp,

  /// End to end: the account's security key isn't the one sent (409
  /// identity_changed): another device made a new one.
  identityChanged,
}

class UserAppError implements Exception {
  const UserAppError(
    this.problem, [
    this.message = '',
    this.details = const {},
  ]);

  final UserAppProblem problem;

  /// The rest of the service's answer (a draft as it is now, on 409).
  final Map<String, Object?> details;

  /// The service's own words, when it gave some (how long to wait, say).
  final String message;

  @override
  String toString() => 'UserAppError(${problem.name}: $message)';
}

/// What an account-level app sends its requests through: this device's
/// signed calls ([DeviceAccount.signed]). Tests give their own.
typedef SignedCall = Future<Map<String, Object?>> Function(
  String method,
  String path, [
  Map<String, Object?>? body,
]);

/// This device's session with the account-level apps: signed requests, each
/// made unique (`request_nonce`, so two alike in the same second aren't
/// taken for a replay), and the service's refusals as [UserAppError].
class UserAppsSession {
  UserAppsSession(this.account, {String Function()? nonce, SignedCall? call})
    : _nonce = nonce ?? _randomNonce,
      _signed = call ?? account.signed;

  /// The session of the device this app runs on.
  factory UserAppsSession.of(BuildContext context) =>
      UserAppsSession(AppScope.scope(context).connections.account);

  final DeviceAccount account;
  final String Function() _nonce;
  final SignedCall _signed;

  bool get signedIn => account.signedIn;

  /// The account's email, to say whose this is.
  String get email => account.email;

  Future<Map<String, Object?>> get(String path) => _call(
    'GET',
    '$path${path.contains('?') ? '&' : '?'}'
        'request_nonce=${_nonce()}',
  );

  Future<Map<String, Object?>> post(String path, Map<String, Object?> body) =>
      _call('POST', path, {...body, 'request_nonce': _nonce()});

  Future<Map<String, Object?>> put(String path, Map<String, Object?> body) =>
      _call('PUT', path, {...body, 'request_nonce': _nonce()});

  Future<Map<String, Object?>> delete(String path) =>
      _call('DELETE', '$path?request_nonce=${_nonce()}');

  Future<Map<String, Object?>> _call(
    String method,
    String path, [
    Map<String, Object?>? body,
  ]) async {
    if (!signedIn) throw const UserAppError(UserAppProblem.notSignedIn);
    try {
      return await _signed(method, path, body);
    } on AccountServiceUnreachable catch (error) {
      throw UserAppError(UserAppProblem.unreachable, error.reason);
    } on AccountServiceError catch (error) {
      throw UserAppError(
        switch ((error.status, error.code)) {
          (401, _) => UserAppProblem.notSignedIn,
          (_, 'device_only') => UserAppProblem.deviceOnly,
          (_, 'accepted') => UserAppProblem.accepted,
          (_, 'epoch') => UserAppProblem.epoch,
          (_, 'exists') => UserAppProblem.exists,
          (_, 'too_far_behind') => UserAppProblem.tooFarBehind,
          (_, 'not_set_up') => UserAppProblem.notSetUp,
          (_, 'identity_changed') => UserAppProblem.identityChanged,
          (404, _) => UserAppProblem.notFound,
          (410, _) => UserAppProblem.gone,
          (_, 'stale') || (409, _) => UserAppProblem.stale,
          (_, 'rate_limited') || (429, _) => UserAppProblem.rateLimited,
          (_, 'storage_full') || (413, _) => UserAppProblem.storageFull,
          (503, _) => UserAppProblem.unavailable,
          _ when error.status >= 500 => UserAppProblem.unavailable,
          _ => UserAppProblem.invalid,
        },
        error.message,
        error.details,
      );
    }
  }
}

final _random = Random.secure();

String _randomNonce() => [
  for (var i = 0; i < 12; i++)
    _random.nextInt(256).toRadixString(16).padLeft(2, '0'),
].join();
