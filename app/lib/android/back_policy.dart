/// Who owns a Back attempt, decided before the gesture changes any UI.
enum AndroidBackOwner { ime, app, system }

/// The captured target of one predictive gesture or three-button Back.
///
/// This object holds no product state. Only its originating coordinator can
/// finish it, at most once. The shared registry must reject stale target
/// identities (including a dismissed/reopened drawer or replaced page).
final class AndroidBackAttempt<T extends Object> {
  AndroidBackAttempt._(
    this._coordinator,
    this.owner,
    this.target,
    this.canClose,
  );

  final Object _coordinator;
  final AndroidBackOwner owner;
  final T? target;

  /// Dismissibility at begin, not permission to pop the enclosing Navigator.
  /// A dirty token is still the target: its commit asks for confirmation.
  final bool canClose;
  bool _finished = false;
}

/// Android's transaction boundary around Mei's shared PhoneBack registry.
///
/// Bind readTarget/canClose/commitTarget to PhoneBack.target,
/// BackTarget.canClose and PhoneBack.commit. The registry owns ordering,
/// lifetime notifications and token/route identities; this adapter never
/// maintains a second navigation stack. Begin/cancel do not mutate it.
///
/// IME dismissal belongs to Android, not a hide-keyboard callback here. Once
/// an attempt belongs to the IME it cannot fall through, even if the keyboard
/// inset reaches zero before the gesture finishes. A root attempt belongs to
/// the system; the shell must allow Android's normal home/back behavior.
final class AndroidBackCoordinator<T extends Object> {
  AndroidBackCoordinator({
    required this.readTarget,
    required this.canClose,
    required this.commitTarget,
  });

  final T? Function() readTarget;
  final bool Function(T) canClose;
  final void Function(T) commitTarget;

  AndroidBackAttempt<T> begin({required bool imeVisible}) {
    if (imeVisible) {
      return AndroidBackAttempt._(this, AndroidBackOwner.ime, null, false);
    }
    final target = readTarget();
    return AndroidBackAttempt._(
      this,
      target == null ? AndroidBackOwner.system : AndroidBackOwner.app,
      target,
      target == null || canClose(target),
    );
  }

  /// Finishes only the captured target, never re-reads the current stack.
  /// Returns its owner so the caller can distinguish app/system/IME handling.
  /// Repeated or foreign completions are programming errors, not another Back.
  AndroidBackOwner finish(
    AndroidBackAttempt<T> attempt, {
    required bool committed,
  }) {
    if (!identical(attempt._coordinator, this) || attempt._finished) {
      throw StateError('Back attempt is foreign or already finished');
    }
    attempt._finished = true;
    if (committed && attempt.owner == AndroidBackOwner.app) {
      commitTarget(attempt.target!);
    }
    return attempt.owner;
  }
}
