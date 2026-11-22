// The clipboard for a code that unlocks something (dms-e2ee §2, H3 as
// Fugo amended it): a new device's code for adding it by pasting. Copied
// marked sensitive, so clipboard managers and clipboard history skip it,
// and cleared when it's done with, unless the person copied something
// since. The platform's half, on the channel `factory/secret-clipboard`:
//
// - `copy` {text: String, seconds: int} → int token: writes [text] as
//   plain text, marked sensitive. On iOS, expiring after [seconds]
//   (`.expirationDate`); on macOS, with the org.nspasteboard Concealed and
//   Transient types; on Android, with ClipDescription.EXTRA_IS_SENSITIVE.
//   Never local-only: Universal Clipboard may carry it between the
//   person's own devices, which is where it's meant to go. The token
//   names this write (Apple's changeCount; on Android, the clip's
//   timestamp).
// - `clear` {token: int} → null: clears the clipboard only if it still
//   holds what that `copy` wrote; otherwise nothing. Dart sends token -1,
//   which no copy gives, to learn whether the half is there.
// Failures are a PlatformException with code `failed`. Where the platform
// has no half (MissingPluginException), there's no copying: [available]
// is false.

import 'package:flutter/services.dart';

class SecretClipboard {
  const SecretClipboard({
    this.channel = const MethodChannel('factory/secret-clipboard'),
  });

  final MethodChannel channel;

  /// How long a copied code stays: as long as it works.
  static const seconds = 30 * 60;

  /// Copies [text]; its token, for [clear], or null where this device
  /// can't copy it safely (then it isn't copied at all).
  Future<int?> copy(String text) async {
    try {
      return await channel.invokeMethod<int>('copy', {
        'text': text,
        'seconds': seconds,
      });
    } on MissingPluginException {
      return null;
    }
  }

  /// Whether this device can copy a code safely.
  Future<bool> get available async {
    try {
      await channel.invokeMethod<void>('clear', {'token': -1});
      return true;
    } on MissingPluginException {
      return false;
    } on PlatformException {
      return true;
    }
  }

  /// Clears what the `copy` that gave [token] wrote, if it's still there.
  Future<void> clear(int token) async {
    try {
      await channel.invokeMethod<void>('clear', {'token': token});
    } on MissingPluginException {
      // Nothing was copied.
    } on PlatformException {
      // Left as it is; it expires or the person copies over it.
    }
  }

  /// What's on the clipboard as text, for pasting a code.
  Future<String?> paste() async =>
      (await Clipboard.getData(Clipboard.kTextPlain))?.text;
}
