// The keys of end-to-end direct messages, in this device's own key store
// (dms-e2ee.md §5.4, E6): the native side keeps opaque secret bytes under a
// name, gives them back and deletes them. It never makes, reads or uses a
// key itself; the app's Rust makes every key and does every signing and all
// of MLS.
//
// The channel, `factory/dm-keys`, for each platform's half:
// - `available` → bool: whether this device can keep these keys at all. On
//   a Mac, true only when a write and read back through the data-protection
//   keychain (kSecUseDataProtectionKeychain, the app's access group) just
//   succeeded: never "the app is signed by a team", and never a fallback to
//   the login keychain or user defaults (a Mac release without its
//   provisioning profile, Claktory Dev ad hoc: false). On iPhone and iPad,
//   the app's keychain; on Android, CredentialStore's Keystore wrap. Asked
//   once per launch; false turns People's messages off on this device.
// - `put` {name: String, bytes: Uint8List} → null: keeps [bytes] under
//   [name], replacing what was there. ThisDeviceOnly and never synchronized
//   (no iCloud Keychain, no Android backup or device transfer), readable
//   whenever the device is unlocked, with no prompt or user presence.
// - `get` {name: String} → Uint8List, or null when nothing is kept under
//   [name]; a PlatformException with code `unreadable` when something is
//   but can't be read (the Keystore key gone, the item damaged, the
//   keychain refusing): never null for that, since absent means "set up
//   again" and unreadable must not.
// - `delete` {name: String} → null: forgets [name]; nothing kept is fine.
// - `folder` → String: the absolute path of a folder for the local store
//   (people/e2ee.dart's `store.db`), made if missing, the same on every
//   launch, this app's own and never backed up or transferred: on iPhone
//   and iPad Application Support/dm, excluded from backup, with file
//   protection until first unlock; on a Mac the app's own Application
//   Support/dm (Claktory and Claktory Dev apart); on Android
//   noBackupFilesDir/dm. Asked only when `available` is true.
// Any other failure is a PlatformException with code `failed` and a
// message for the log.
//
// Names are [DmKeyName]'s: lowercase ASCII, at most 32 characters.

import 'package:flutter/services.dart';

/// What's kept: this person's identity key, this device's leaf key, and
/// the local store's seal key (32 bytes each, made in Rust).
enum DmKeyName {
  identity('identity'),
  leaf('leaf'),
  store('store');

  const DmKeyName(this.wire);

  /// Its name on the channel.
  final String wire;
}

/// A key kept under a name can't be read: never treated as absent.
class DmKeyUnreadable implements Exception {
  const DmKeyUnreadable(this.name, this.message);

  final DmKeyName name;
  final String message;

  @override
  String toString() => "The ${name.wire} key can't be read: $message";
}

/// This device's key store for direct messages, through `factory/dm-keys`.
class DmKeys {
  const DmKeys({this.channel = const MethodChannel('factory/dm-keys')});

  final MethodChannel channel;

  /// Whether this device can keep the keys; false where the platform has no
  /// half of the channel yet, too.
  Future<bool> available() async {
    try {
      return await channel.invokeMethod<bool>('available') ?? false;
    } on MissingPluginException {
      return false;
    }
  }

  Future<void> put(DmKeyName name, Uint8List bytes) =>
      channel.invokeMethod<void>('put', {'name': name.wire, 'bytes': bytes});

  /// What's kept under [name], or null when nothing is; throws
  /// [DmKeyUnreadable] when something is but can't be read.
  Future<Uint8List?> get(DmKeyName name) async {
    try {
      return await channel.invokeMethod<Uint8List>('get', {'name': name.wire});
    } on PlatformException catch (error) {
      if (error.code == 'unreadable') {
        throw DmKeyUnreadable(name, error.message ?? '');
      }
      rethrow;
    }
  }

  Future<void> delete(DmKeyName name) =>
      channel.invokeMethod<void>('delete', {'name': name.wire});

  /// The folder the local store lives in.
  Future<String> folder() async =>
      await channel.invokeMethod<String>('folder') ??
      (throw PlatformException(code: 'failed', message: 'No folder.'));
}
