import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

/// What Back closes on a phone, in order (docs/IDENTITY.md, "Phone"): the
/// newest sheet first, then the drawer, then the page on top; with none of
/// them, nothing here (the conversation is the root, and Back belongs to
/// the system).
///
/// The iPhone has no Back of its own: a sheet is dragged down, the drawer
/// swiped closed, a page swiped back, each by its own gesture. This keeps
/// the order for a platform that has one (Android's Back: the button, the
/// gesture, a keyboard's Esc), whose host reads [PhoneBack.target] when
/// Back begins and [PhoneBack.commit]s it when Back completes.
///
/// Nothing here is state of the person's: like which page shows, it's this
/// window's own and goes nowhere.

/// Something over the screen that Back closes before what's under it: a
/// sheet, say. Register it with [PhoneBack.register] while it's up.
abstract interface class BackToken {
  /// Whether it closes as it is. False when closing would drop something
  /// (a form with changes): Back still reaches [back], which asks first.
  bool get canClose;

  /// Closes it, as the person dismissing it would (dragging a sheet down):
  /// asking first when [canClose] is false.
  void back();
}

/// A [BackToken]'s registration.
abstract interface class BackHandle {
  /// Its [BackToken.canClose] changed.
  void changed();

  /// It's going: Back no longer reaches it. Safe to call more than once.
  void release();
}

/// What Back would close: one occurrence of it. A target read when Back
/// begins and committed when it completes acts only if it's still what
/// Back would close then ([PhoneBack.commit]); a sheet closed and opened
/// again, the drawer closed and opened again, or a page replaced by one
/// with the same id, is another target.
sealed class BackTarget {
  const BackTarget();

  /// Whether it closes as it is (see [BackToken.canClose]). Committing it
  /// still reaches it when false: it asks first.
  bool get canClose;
}

/// A sheet, or anything else registered with [PhoneBack.register].
final class TokenTarget extends BackTarget {
  const TokenTarget._(this.token, this._serial);

  final BackToken token;
  final int _serial;

  @override
  bool get canClose => token.canClose;

  @override
  bool operator ==(Object other) =>
      other is TokenTarget &&
      identical(other.token, token) &&
      other._serial == _serial;

  @override
  int get hashCode => Object.hash(identityHashCode(token), _serial);

  @override
  String toString() => 'TokenTarget($_serial)';
}

/// The drawer, as opened the [generation]th time.
final class DrawerTarget extends BackTarget {
  const DrawerTarget(this.generation);

  final int generation;

  @override
  bool get canClose => true;

  @override
  bool operator ==(Object other) =>
      other is DrawerTarget && other.generation == generation;

  @override
  int get hashCode => Object.hash(DrawerTarget, generation);

  @override
  String toString() => 'DrawerTarget($generation)';
}

/// The page on top of the phone's pages: [id] says which ('route:…',
/// 'page:settings'…), [serial] which time it was pushed.
final class PageTarget extends BackTarget {
  const PageTarget({
    required this.id,
    required this.serial,
    this.fullscreenDialog = false,
  });

  final String id;
  final int serial;

  /// It rose from the bottom as a task of its own, closed with Cancel or
  /// Done on an iPhone rather than Back (a long form).
  final bool fullscreenDialog;

  @override
  bool get canClose => true;

  @override
  bool operator ==(Object other) =>
      other is PageTarget && other.id == id && other.serial == serial;

  @override
  int get hashCode => Object.hash(id, serial);

  @override
  String toString() => 'PageTarget($id #$serial)';
}

/// A phone's Back: what it would close now ([target]), in order, and
/// closing it ([commit]). Listen to it to hear when [target] (or its
/// [BackTarget.canClose]) changes. The phone's drawer owns it
/// ([PhoneBackScope]); sheets register with it.
class PhoneBack extends ChangeNotifier {
  PhoneBack({
    required this._drawerOpen,
    required this._closeDrawer,
    required this._topPage,
    required this._popPage,
  });

  final bool Function() _drawerOpen;
  final VoidCallback _closeDrawer;
  final PageTarget? Function() _topPage;
  final VoidCallback _popPage;

  /// The phone's Back above [context], if it's on a phone. It doesn't make
  /// [context] depend on it: listen to it.
  static PhoneBack? maybeOf(BuildContext context) =>
      context.getInheritedWidgetOfExactType<PhoneBackScope>()?.back;

  /// What's registered, oldest first.
  final _tokens = <_Registration>[];
  var _serial = 0;

  /// How many times the drawer has opened; the one open now.
  var _generation = 0;
  var _wasOpen = false;

  BackTarget? _told;
  bool? _toldCanClose;
  var _telling = false;
  var _disposed = false;

  /// Registers [token] over everything registered before it, until the
  /// handle's released.
  BackHandle register(BackToken token) {
    final registration = _Registration(this, token, ++_serial);
    _tokens.add(registration);
    _changed();
    return registration;
  }

  /// What Back would close now: the newest token, else the open drawer,
  /// else the page on top; null at the root.
  BackTarget? get target {
    if (_tokens.isNotEmpty) {
      final top = _tokens.last;
      return TokenTarget._(top.token, top.serial);
    }
    if (_drawerOpen()) return DrawerTarget(_generation);
    return _topPage();
  }

  /// Closes [target] if it's still what Back would close ([target] now),
  /// and says whether it did: a target that's gone or that something has
  /// covered since does nothing. A token is reached even when it can't
  /// close as it is ([BackToken.back] asks first).
  bool commit(BackTarget target) {
    update();
    if (target != this.target) return false;
    switch (target) {
      case TokenTarget(:final token):
        token.back();
      case DrawerTarget():
        _closeDrawer();
      case PageTarget():
        _popPage();
    }
    return true;
  }

  /// The drawer or the pages changed: tells listeners if [target] did.
  void update() {
    final open = _drawerOpen();
    if (open && !_wasOpen) _generation++;
    _wasOpen = open;
    _changed();
  }

  void _release(_Registration registration) {
    if (_tokens.remove(registration)) _changed();
  }

  /// Tells listeners when what [target] says changed, after the frame when
  /// it changed during a build (a sheet registering as it's built).
  void _changed() {
    if (_disposed) return;
    if (SchedulerBinding.instance.schedulerPhase ==
        SchedulerPhase.persistentCallbacks) {
      if (_telling) return;
      _telling = true;
      SchedulerBinding.instance.addPostFrameCallback((_) {
        _telling = false;
        _tell();
      });
      return;
    }
    _tell();
  }

  void _tell() {
    if (_disposed) return;
    final open = _drawerOpen();
    if (open && !_wasOpen) _generation++;
    _wasOpen = open;
    final now = target;
    final canClose = now?.canClose;
    if (now == _told && canClose == _toldCanClose) return;
    _told = now;
    _toldCanClose = canClose;
    notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _tokens.clear();
    super.dispose();
  }
}

class _Registration implements BackHandle {
  _Registration(this.owner, this.token, this.serial);

  final PhoneBack owner;
  final BackToken token;
  final int serial;

  @override
  void changed() => owner._changed();

  @override
  void release() => owner._release(this);
}

/// Gives the widgets below a phone's [PhoneBack].
class PhoneBackScope extends InheritedWidget {
  const PhoneBackScope({super.key, required this.back, required super.child});

  final PhoneBack back;

  @override
  bool updateShouldNotify(PhoneBackScope old) => old.back != back;
}

/// A [BackToken] from two functions.
class CallbackBackToken implements BackToken {
  CallbackBackToken({required this.onBack, this._canClose});

  final VoidCallback onBack;
  final bool Function()? _canClose;

  @override
  bool get canClose => _canClose?.call() ?? true;

  @override
  void back() => onBack();
}
