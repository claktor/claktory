// The clipboard for a code that unlocks something (dms-e2ee §2, H3 as
// Fugo amended it): a new device's code for adding it by pasting, and a
// pipe invite's code (row 166). Copied marked sensitive, so clipboard
// managers and clipboard history skip it, and cleared when it's done with,
// unless the person copied something since. The platform's half, on the
// channel `factory/secret-clipboard`:
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

import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart'
    show AppLifecycleListener, AppLifecycleState, WidgetsBinding;

class SecretClipboard {
  const SecretClipboard({
    this.channel = const MethodChannel('factory/secret-clipboard'),
  });

  final MethodChannel channel;

  /// How long a copied code stays: as long as it works.
  static const seconds = 30 * 60;

  /// Copies [text]; its token, for [clear], or null where this device
  /// can't copy it safely (then it isn't copied at all). [lasts]: how long
  /// it works, when that isn't [seconds] (a pipe invite's 10 minutes).
  Future<int?> copy(String text, {int lasts = seconds}) async {
    try {
      return await channel.invokeMethod<int>('copy', {
        'text': text,
        'seconds': lasts,
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

/// From launch, for the app's life: clears any of this app's codes the
/// clipboard still holds past their time (Fugo's row 181), at launch and on
/// each return to the app. The platform's `sweep` reads only what's on the
/// clipboard (its label and the expiry it carries), never its text, and
/// clears it only when it's this app's and expired: a code copied before
/// the app was stopped, or one due while it was away. Where the platform
/// has no `sweep`, nothing.
/// Answers how to stop them (for tests).
VoidCallback startClipboardSweeps({
  MethodChannel channel = const MethodChannel('factory/secret-clipboard'),
}) {
  Future<void> sweep() async {
    try {
      await channel.invokeMethod<void>('sweep');
    } on MissingPluginException {
      // No sweep here (an older native half, or none).
    } on PlatformException {
      // Left for the next return.
    }
  }

  // Once its window has the focus Android asks for.
  void soon() => Timer(const Duration(milliseconds: 500), () {
    unawaited(sweep());
  });
  soon();
  final sweeps = AppLifecycleListener(onResume: soon);
  return sweeps.dispose;
}

/// Clears [token]'s copy on [clipboard] once [lasts] is over, if it's
/// still there. Android lets an app check its clipboard only while it's in
/// front, so a clear due while it isn't (the person in their messenger)
/// waits for it to come back, and is tried again then (Zucchero's row 181).
void clearWhenDue(SecretClipboard clipboard, int token, Duration lasts) {
  final clear = _DueClear(clipboard, token);
  _clears.add(clear);
  _onResume ??= AppLifecycleListener(
    // Once its window has the focus Android asks for.
    onResume: () => Timer(const Duration(milliseconds: 500), _clearDue),
  );
  Timer(lasts, () {
    clear.due = true;
    _clearDue();
  });
}

class _DueClear {
  _DueClear(this.clipboard, this.token);

  final SecretClipboard clipboard;
  final int token;
  var due = false;
}

final _clears = <_DueClear>[];
AppLifecycleListener? _onResume;

/// Clears what's due; in front, that's settled (the platform could
/// check), so it's let go; away, it's kept for the next return.
void _clearDue() {
  final due = [
    for (final clear in _clears)
      if (clear.due) clear,
  ];
  for (final clear in due) {
    unawaited(clear.clipboard.clear(clear.token));
  }
  final state = WidgetsBinding.instance.lifecycleState;
  if (state == null || state == AppLifecycleState.resumed) {
    _clears.removeWhere(due.contains);
  }
  if (_clears.isEmpty) {
    _onResume?.dispose();
    _onResume = null;
  }
}
