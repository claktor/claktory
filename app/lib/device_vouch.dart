import 'dart:async';
import 'dart:developer' as developer;

import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'device_account.dart';
import 'device_kind.dart' show thisDeviceName;
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'protocol/factory/v1/factory.pb.dart';
import 'update.dart' show AppBuild;
import 'your_claktories.dart' show accountProblem, thisDevicePlatform;

/// A Mac's app signed in to the person's Claktory account through its own
/// Claktory (#529), so their other Claktories take it as theirs with no
/// second sign-in: while this Mac's Claktory is signed in and the app
/// isn't, the app makes an account key, keeps it in the keychain
/// ([AccountKeyVault]), asks its Claktory to vouch for it (VouchDevice)
/// and signs in with the one-time code it gets back, signed by that key.
///
/// It does so by itself unless the person signed this device out or
/// another of their devices removed it ([DeviceAccount.autoVouch], Fugo's
/// condition 5): then [offered] says so, and [vouchNow] does it once on
/// the person's tap ([SignInThroughThisMac]). An account service that
/// can't vouch yet leaves the app as it was, with nothing shown, until it
/// starts again.
class LocalVouch extends ChangeNotifier {
  LocalVouch({
    required this.account,
    required this.local,
    required this.changes,
    required this.others,
    this.vault = const AccountKeyVault(),
    this.keys = const RustAccountKeys(),
    Future<String> Function()? name,
    String? version,
  }) : _name = name ?? thisDeviceName,
       _version = version ?? AppBuild.current().version {
    changes.addListener(_follow);
    account.addListener(_check);
    _follow();
  }

  /// The one the app made, for [SignInThroughThisMac]; null on devices
  /// without a Claktory of their own.
  static LocalVouch? current;

  final DeviceAccount account;

  /// This Mac's own Claktory's connection; null when it runs none.
  final Backend? Function() local;

  /// The Claktories on other machines: those that turned this device away
  /// until it signs in try again once it is.
  final Iterable<Backend> Function() others;

  final AccountKeyVault vault;
  final AccountKeys keys;

  /// What says [local] may be another connection now.
  final Listenable changes;
  final Future<String> Function() _name;
  final String _version;

  Backend? _watched;

  /// Whether the keychain can keep the key: asked once.
  bool? _vaultWorks;

  /// Asking now.
  var _busy = false;

  /// The service can't vouch (it's from before it), or this app may not
  /// ask: nothing more until the app starts again.
  var _stopped = false;

  /// Failures in a row, for how long to wait before trying again.
  var _failures = 0;
  Timer? _again;
  static const _waits = [
    Duration(minutes: 1),
    Duration(minutes: 5),
    Duration(minutes: 15),
    Duration(hours: 1),
  ];

  /// What went wrong with the person's last [vouchNow], in words; null
  /// when it worked or they haven't tried.
  String? get problem => _problem;
  String? _problem;

  /// Signing in now.
  bool get busy => _busy;

  /// Whether this Mac's Claktory is signed in to the account.
  bool get _localSignedIn {
    final backend = local();
    return backend != null &&
        backend.connected.value &&
        backend.workspace.workspace.account.state ==
            ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN;
  }

  /// The person can sign the app in through this Mac's Claktory with one
  /// click: it's signed in, the app isn't, and won't by itself.
  bool get offered =>
      _localSignedIn &&
      !account.signedIn &&
      !account.autoVouch &&
      _vaultWorks != false &&
      !_stopped;

  /// The account's email, as this Mac's Claktory is signed in with it.
  String get email {
    final backend = local();
    return backend == null ? '' : backend.workspace.workspace.account.email;
  }

  void _follow() {
    final backend = local();
    if (!identical(backend, _watched)) {
      _watched?.connected.removeListener(_check);
      _watched?.workspace.removeListener(_check);
      _watched = backend
        ?..connected.addListener(_check)
        ..workspace.addListener(_check);
    }
    _check();
  }

  bool _wasOffered = false;

  void _check() {
    if (offered != _wasOffered) {
      _wasOffered = offered;
      notifyListeners();
    }
    if (_busy || _stopped || _again != null) return;
    if (!_localSignedIn || account.signedIn || !account.autoVouch) return;
    if (_vaultWorks == false) return;
    unawaited(_vouch());
  }

  /// Signs the app in through this Mac's Claktory once, on the person's
  /// click; [problem] says what went wrong.
  Future<void> vouchNow() async {
    if (_busy) return;
    _problem = null;
    await _vouch(asked: true);
  }

  Future<void> _vouch({bool asked = false}) async {
    final backend = local();
    if (backend == null) return;
    _busy = true;
    notifyListeners();
    try {
      final works = _vaultWorks ??= await vault.available();
      if (!works) {
        _log("the keychain can't keep an account key here");
        return;
      }
      // Asked by the person after this device was removed, its old key
      // can't sign in again: a new one.
      var key = account.vouchKey(fresh: asked && account.removed);
      for (var tries = 0; tries < 2; tries++) {
        await vault.write(key);
        final vouched = await backend.vouchDevice(
          accountKey: b64(await keys.publicKey(key)),
          name: await _name(),
          platform: thisDevicePlatform,
          version: _version,
        );
        switch (vouched.refused) {
          case '':
            try {
              await account.signInVouched(
                key: key,
                service: vouched.service,
                requestId: vouched.requestId,
                code: vouched.code,
                email: vouched.email,
              );
            } on AccountServiceError catch (error) {
              // The key was another account's, or removed and the person
              // asked: a new one, once more.
              if (tries == 0 &&
                  error.status == 403 &&
                  (error.code == 'key_of_another_account' ||
                      (asked && error.code == 'device_removed'))) {
                key = account.vouchKey(fresh: true);
                continue;
              }
              rethrow;
            }
            _failures = 0;
            _log('signed in through this Mac\'s Claktory');
            // Those that turned this device away try again now.
            for (final other in others()) {
              if (other.signInRequired.value != null) other.retry();
            }
            return;
          case 'key_of_another_account' when tries == 0:
            key = account.vouchKey(fresh: true);
            continue;
          case 'device_removed' when asked && tries == 0:
            key = account.vouchKey(fresh: true);
            continue;
          case 'unavailable':
            // The service can't vouch yet: as before, nothing shown.
            _stopped = true;
            _log('the account service can\'t vouch yet');
            return;
          case 'device_removed':
            account.turnOffAutoVouch();
            if (asked) _problem = vouched.message;
            return;
          default:
            if (asked) {
              _problem = vouched.message.isNotEmpty
                  ? vouched.message
                  : _fallback;
            }
            _later();
            return;
        }
      }
    } on RequestFailure catch (failure) {
      _log('couldn\'t ask this Mac\'s Claktory: ${failure.message}');
      if (failure.code == ErrorCode.ERROR_CODE_UNAUTHORIZED) _stopped = true;
      if (asked) _problem = failure.message;
      if (!_stopped) _later();
    } on AccountServiceError catch (error) {
      _log('the account service refused: $error');
      if (asked) _problem = accountProblem(error, account.service);
      // device_removed turned signing in by itself off.
      if (account.autoVouch) _later();
    } on AccountServiceUnreachable catch (error) {
      _log('the account service can\'t be reached: $error');
      if (asked) _problem = accountProblem(error, account.service);
      _later();
    } catch (error) {
      _log('couldn\'t sign in: $error');
      if (asked) _problem = _fallback;
      _later();
    } finally {
      _busy = false;
      _wasOffered = offered;
      notifyListeners();
    }
  }

  /// Tries again by itself after a while, longer each time.
  void _later() {
    final wait = _waits[_failures.clamp(0, _waits.length - 1)];
    _failures++;
    _again?.cancel();
    _again = Timer(wait, () {
      _again = null;
      _check();
    });
  }

  /// What a failed click says when nothing said why (Trish).
  static const _fallback =
      "Couldn't sign in this way. Try again, or use your email below.";

  void _log(String message) =>
      developer.log('device sign-in: $message', name: 'factory');

  @override
  void dispose() {
    _again?.cancel();
    changes.removeListener(_follow);
    account.removeListener(_check);
    _watched?.connected.removeListener(_check);
    _watched?.workspace.removeListener(_check);
    if (identical(current, this)) current = null;
    super.dispose();
  }
}

/// "Use this account", this Mac's Claktory's: one click, where this device
/// would otherwise sign in with an emailed code, while [LocalVouch.offered]
/// (signed out by the person, or removed by another device). Nothing where
/// it isn't offered.
class SignInThroughThisMac extends StatelessWidget {
  const SignInThroughThisMac({super.key, this.vouch, this.refused});

  /// Tests give one; the app's by default.
  final LocalVouch? vouch;

  /// An account the Claktory asking turns away: nothing when it's the one
  /// this Mac's Claktory would vouch with (Abbacchio row 1541).
  final String? refused;

  @override
  Widget build(BuildContext context) {
    final vouch = this.vouch ?? LocalVouch.current;
    if (vouch == null) return const SizedBox.shrink();
    return ListenableBuilder(
      listenable: vouch,
      builder: (context, _) {
        if (!vouch.offered) return const SizedBox.shrink();
        final refused = this.refused?.trim().toLowerCase();
        if (refused != null &&
            refused.isNotEmpty &&
            vouch.email.trim().toLowerCase() == refused) {
          return const SizedBox.shrink();
        }
        final palette = Palette.of(context);
        final type = TypeScale.of(context);
        final email = vouch.email;
        final problem = vouch.problem;
        return Padding(
          padding: const EdgeInsets.only(bottom: Space.l),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                email.isEmpty
                    ? "This Mac's Claktory is signed in. Use its account "
                          'here too, with no code.'
                    : "This Mac's Claktory is signed in as $email. Use that "
                          'account here too, with no code.',
                style: type.body.copyWith(color: palette.textMuted),
              ),
              const SizedBox(height: Space.s),
              Align(
                alignment: Alignment.centerLeft,
                child: Button(
                  label: 'Use this account',
                  primary: true,
                  onPressed: vouch.busy
                      ? null
                      : () => unawaited(vouch.vouchNow()),
                ),
              ),
              if (problem != null) ...[
                const SizedBox(height: Space.s),
                Text(
                  problem,
                  style: type.caption.copyWith(color: palette.danger),
                ),
              ],
            ],
          ),
        );
      },
    );
  }
}
