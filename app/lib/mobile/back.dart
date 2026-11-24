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

/// Optional cleanup for a surface belonging to a window that was replaced.
/// This is not Back and never runs for gesture cancellation or ordinary release.
abstract interface class RetirableBackToken implements BackToken {
  void retired();
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
/// [BackTarget.canClose]) changes. Android's window owns its root; a standalone
/// phone drawer owns a local one. [PhoneBackScope] lends it to surfaces.
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

  /// A persistent window registry. Drawer/page providers attach while their
  /// owner is visible; the platform dispatcher itself survives layout changes.
  factory PhoneBack.root() => PhoneBack(
    drawerOpen: () => false,
    closeDrawer: () {},
    topPage: () => null,
    popPage: () {},
  );

  late final _owners = _BackOwner(this);
  final _navigations = <_BackNavigation>[];
  _BackNavigation? _lastNavigation;

  /// Selects one visible child region, returning a borrowed view of this SAME
  /// registry. Its outgoing sibling's navigation leases stop participating
  /// even while AnimatedSwitcher keeps it alive. Overlays belong to the whole
  /// window and survive its resize; changing the root window retires them.
  /// No second stack/dispatcher is created. Readers must not dispose the view.
  PhoneBack branch(Object key) => _branch(_owners, key);

  PhoneBack _branch(_BackOwner parent, Object key) {
    if (parent.child?.key == key) return parent.child!.view;
    final outgoing = parent.child;
    final next = _BackOwner(this, parent: parent, key: key);
    parent.child = next;
    if (outgoing != null) {
      final retired = [
        ..._tokens.where((entry) => entry.scope.within(outgoing)),
        ..._navigationTokens.where((entry) => entry.scope.within(outgoing)),
      ];
      _tokens.removeWhere((entry) => entry.scope.within(outgoing));
      _navigationTokens.removeWhere((entry) => entry.scope.within(outgoing));
      _navigations.removeWhere((entry) => entry.scope.within(outgoing));
      for (final entry in retired) {
        entry.retire();
      }
    }
    if (parent.active) update();
    return next.view;
  }

  /// A drawer supplies its existing state to a lifted registry. Release when
  /// it leaves; a new active owner replaces the going window's callbacks.
  BackHandle attachNavigation({
    required bool Function() drawerOpen,
    required VoidCallback closeDrawer,
    required PageTarget? Function() topPage,
    required VoidCallback popPage,
  }) => _attachNavigation(
    _owners,
    drawerOpen: drawerOpen,
    closeDrawer: closeDrawer,
    topPage: topPage,
    popPage: popPage,
  );

  BackHandle _attachNavigation(
    _BackOwner scope, {
    required bool Function() drawerOpen,
    required VoidCallback closeDrawer,
    required PageTarget? Function() topPage,
    required VoidCallback popPage,
  }) {
    if (!scope.active) return const _ReleasedBackHandle();
    final navigation = _BackNavigation(
      this,
      scope,
      drawerOpen,
      closeDrawer,
      topPage,
      popPage,
    );
    _navigations.add(navigation);
    update();
    return navigation;
  }

  _BackNavigation? get _navigation {
    for (final navigation in _navigations.reversed) {
      if (navigation.scope.active) return navigation;
    }
    return null;
  }

  bool get _open => _navigation?.drawerOpen() ?? _drawerOpen();

  void _syncNavigation() {
    final navigation = _navigation;
    if (identical(navigation, _lastNavigation)) return;
    _lastNavigation = navigation;
    // A new provider whose drawer is already open is not the old drawer.
    _generation++;
    _wasOpen = false;
  }

  /// The phone's Back above [context], if it's on a phone. It doesn't make
  /// [context] depend on it: listen to it.
  static PhoneBack? maybeOf(BuildContext context) =>
      context.getInheritedWidgetOfExactType<PhoneBackScope>()?.back;

  /// What's registered, oldest first.
  final _tokens = <_Registration>[];
  final _navigationTokens = <_Registration>[];
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
    return _register(token, _owners);
  }

  /// Structural surfaces below overlays: wide expanded panes/open pages and
  /// the Claktories list/add route. A rebuilt page must not cover a surviving
  /// dialog's Back target just because the window changed size.
  BackHandle registerNavigation(BackToken token) =>
      _register(token, _owners, navigation: true);

  BackHandle _register(
    BackToken token,
    _BackOwner scope, {
    bool navigation = false,
  }) {
    if (!scope.active) return const _ReleasedBackHandle();
    final registration = _Registration(this, token, ++_serial, scope);
    (navigation ? _navigationTokens : _tokens).add(registration);
    _changed();
    return registration;
  }

  /// What Back closes: newest overlay, then newest structural navigation
  /// surface, then the open drawer, then its top page; null at the root.
  BackTarget? get target {
    if (_tokens.isNotEmpty) {
      final top = _tokens.last;
      return TokenTarget._(top.token, top.serial);
    }
    if (_navigationTokens.isNotEmpty) {
      final top = _navigationTokens.last;
      return TokenTarget._(top.token, top.serial);
    }
    if (_open) return DrawerTarget(_generation);
    final navigation = _navigation;
    return navigation == null ? _topPage() : navigation.pageTarget;
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
        final navigation = _navigation;
        navigation == null ? _closeDrawer() : navigation.closeDrawer();
      case PageTarget():
        final navigation = _navigation;
        navigation == null ? _popPage() : navigation.popPage();
    }
    return true;
  }

  /// The drawer or the pages changed: tells listeners if [target] did.
  void update() {
    _syncNavigation();
    final open = _open;
    if (open && !_wasOpen) _generation++;
    _wasOpen = open;
    _changed();
  }

  void _release(_Registration registration) {
    final removed = _tokens.remove(registration);
    final navigationRemoved = _navigationTokens.remove(registration);
    if (removed || navigationRemoved) _changed();
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
    _syncNavigation();
    final open = _open;
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
    _navigationTokens.clear();
    _navigations.clear();
    _owners.child = null;
    super.dispose();
  }
}

class _Registration implements BackHandle {
  _Registration(this.owner, this.token, this.serial, this.scope);

  final PhoneBack owner;
  final BackToken token;
  final int serial;
  final _BackOwner scope;
  var _released = false;
  var _retired = false;

  void retire() {
    if (_released || _retired) return;
    _retired = true;
    final surface = token;
    if (surface is! RetirableBackToken) return;
    SchedulerBinding.instance
      ..addPostFrameCallback((_) {
        if (!_released) surface.retired();
      })
      ..ensureVisualUpdate();
  }

  @override
  void changed() {
    if (!_released && scope.active) owner._changed();
  }

  @override
  void release() {
    _released = true;
    owner._release(this);
  }
}

/// An owner is only a lifetime/visibility lease, never navigation state.
class _BackOwner {
  _BackOwner(this.root, {this.parent, this.key});
  final PhoneBack root;
  final _BackOwner? parent;
  final Object? key;
  _BackOwner? child;
  late final PhoneBack view = _ScopedPhoneBack(this);

  bool get active =>
      !root._disposed &&
      (parent == null || (parent!.active && identical(parent!.child, this)));

  // Overlay leases belong to the window, not its phone/list/add region. A
  // modal above the window remains the same surface while it resizes.
  _BackOwner get window {
    var owner = this;
    while (owner.parent?.parent != null) {
      owner = owner.parent!;
    }
    return owner;
  }

  bool within(_BackOwner ancestor) {
    for (_BackOwner? owner = this; owner != null; owner = owner.parent) {
      if (identical(owner, ancestor)) return true;
    }
    return false;
  }
}

class _ScopedPhoneBack extends PhoneBack {
  _ScopedPhoneBack(this.scope)
    : super(
        drawerOpen: () => false,
        closeDrawer: () {},
        topPage: () => null,
        popPage: () {},
      );
  final _BackOwner scope;
  PhoneBack get root => scope.root;

  @override
  PhoneBack branch(Object key) => root._branch(scope, key);
  @override
  BackTarget? get target => scope.active ? root.target : null;
  @override
  BackHandle register(BackToken token) => scope.active
      ? root._register(token, scope.window)
      : const _ReleasedBackHandle();
  @override
  BackHandle registerNavigation(BackToken token) =>
      root._register(token, scope, navigation: true);
  @override
  bool commit(BackTarget target) => scope.active && root.commit(target);
  @override
  void update() {
    if (scope.active) root.update();
  }

  @override
  void addListener(VoidCallback listener) => root.addListener(listener);
  @override
  void removeListener(VoidCallback listener) => root.removeListener(listener);
  @override
  bool get hasListeners => root.hasListeners;
  @override
  BackHandle attachNavigation({
    required bool Function() drawerOpen,
    required VoidCallback closeDrawer,
    required PageTarget? Function() topPage,
    required VoidCallback popPage,
  }) => root._attachNavigation(
    scope,
    drawerOpen: drawerOpen,
    closeDrawer: closeDrawer,
    topPage: topPage,
    popPage: popPage,
  );
}

class _BackNavigation implements BackHandle {
  _BackNavigation(
    this.root,
    this.scope,
    this.drawerOpen,
    this.closeDrawer,
    this.topPage,
    this.popPage,
  );
  final PhoneBack root;
  final _BackOwner scope;
  final bool Function() drawerOpen;
  final VoidCallback closeDrawer;
  final PageTarget? Function() topPage;
  final VoidCallback popPage;
  PageTarget? _readPage;
  PageTarget? _mappedPage;

  PageTarget? get pageTarget {
    final page = topPage();
    if (page == null) {
      _readPage = null;
      _mappedPage = null;
    } else if (page != _readPage) {
      _readPage = page;
      // Local page serials can restart in another window/provider. The root
      // serial prevents a captured old page from popping a new identical id.
      _mappedPage = PageTarget(
        id: page.id,
        serial: ++root._serial,
        fullscreenDialog: page.fullscreenDialog,
      );
    }
    return _mappedPage;
  }

  @override
  void changed() {
    if (scope.active) root.update();
  }

  @override
  void release() {
    if (root._navigations.remove(this)) root.update();
  }
}

class _ReleasedBackHandle implements BackHandle {
  const _ReleasedBackHandle();
  @override
  void changed() {}
  @override
  void release() {}
}

/// Gives the widgets below a phone's [PhoneBack].
class PhoneBackScope extends InheritedWidget {
  const PhoneBackScope({super.key, required this.back, required super.child});

  final PhoneBack back;

  @override
  bool updateShouldNotify(PhoneBackScope old) => old.back != back;
}

/// A [BackToken] from two functions.
class CallbackBackToken implements RetirableBackToken {
  CallbackBackToken({required this.onBack, this._canClose, this.onRetired});

  final VoidCallback onBack;
  final bool Function()? _canClose;

  /// Remove the going window's overlay without committing Back or a form.
  /// Deferred until after the frame; normal close/release cancels it.
  final VoidCallback? onRetired;

  @override
  void retired() => onRetired?.call();

  @override
  bool get canClose => _canClose?.call() ?? true;

  @override
  void back() => onBack();
}
