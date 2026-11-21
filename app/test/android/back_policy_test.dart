import 'package:factory_app/android/back_policy.dart';
import 'package:flutter_test/flutter_test.dart';

/// A fake of the agreed shared seam, not a replacement production registry.
/// Tokens are occurrence identities; removing one invalidates stale commits.
class _Target {
  _Target(this.name, {this.canClose = true});
  final String name;
  bool canClose;
}

class _BackHost {
  final tokens = <_Target>[];
  final pages = <_Target>[];
  _Target? drawer;
  final committed = <String>[];
  var confirmations = 0;
  var draft = 'Unsent 日本語 👩🏽‍💻';
  final backendLayout = <String>['conversation', 'viewer'];

  _Target? get target => tokens.lastOrNull ?? drawer ?? pages.lastOrNull;

  void commit(_Target target) {
    // A stale target must not dismiss a different occurrence, nor a lower
    // layer if another surface appeared during the gesture.
    if (!identical(target, this.target)) return;
    committed.add(target.name);
    if (!target.canClose) {
      confirmations++;
    } else if (tokens.contains(target)) {
      tokens.remove(target);
    } else if (identical(drawer, target)) {
      drawer = null;
    } else {
      pages.remove(target);
    }
  }

  late final coordinator = AndroidBackCoordinator<_Target>(
    readTarget: () => target,
    canClose: (target) => target.canClose,
    commitTarget: commit,
  );

  void back() =>
      coordinator.finish(coordinator.begin(imeVisible: false), committed: true);
}

void main() {
  test('one three-button Back acts on only the top layer in order', () {
    final host = _BackHost()
      ..pages.add(_Target('viewer'))
      ..drawer = _Target('drawer')
      ..tokens.addAll([_Target('sheet'), _Target('selection')]);
    for (final expected in ['selection', 'sheet', 'drawer', 'viewer']) {
      host.back();
      expect(host.committed.last, expected);
    }
    final root = host.coordinator.begin(imeVisible: false);
    expect(root.owner, AndroidBackOwner.system);
    host.coordinator.finish(root, committed: true);
    expect(host.committed, ['selection', 'sheet', 'drawer', 'viewer']);
    expect(host.backendLayout, ['conversation', 'viewer']);
    expect(host.draft, 'Unsent 日本語 👩🏽‍💻');
  });

  for (final surface in [
    'selection',
    'menu',
    'dialog',
    'sheet',
    'drawer',
    'page',
  ]) {
    test('model cancellation preserves $surface and draft', () {
      final host = _BackHost();
      final target = _Target(surface);
      if (surface == 'drawer') {
        host.drawer = target;
      } else if (surface == 'page') {
        host.pages.add(target);
      } else {
        host.tokens.add(target);
      }
      final attempt = host.coordinator.begin(imeVisible: false);
      expect(attempt.target, same(target));
      expect(host.target, same(target), reason: 'begin is read-only');
      host.coordinator.finish(attempt, committed: false);
      expect(host.target, same(target));
      expect(host.committed, isEmpty);
      expect(host.draft, 'Unsent 日本語 👩🏽‍💻');
      expect(host.backendLayout, ['conversation', 'viewer']);
    });
  }

  test('IME owns the whole attempt even after its inset disappears', () {
    final host = _BackHost()..pages.add(_Target('settings'));
    final keyboardBack = host.coordinator.begin(imeVisible: true);
    // In the host the OS now dismisses the keyboard; Flutter may observe a
    // zero inset before receiving completion. Do not re-read that inset.
    expect(
      host.coordinator.finish(keyboardBack, committed: true),
      AndroidBackOwner.ime,
    );
    expect(host.pages.single.name, 'settings');
    expect(host.committed, isEmpty);
    host.back();
    expect(host.pages, isEmpty, reason: 'a separate Back may now pop');
  });

  test('root captured before a new page never closes that page', () {
    final host = _BackHost();
    final root = host.coordinator.begin(imeVisible: false);
    host.pages.add(_Target('new page'));
    expect(
      host.coordinator.finish(root, committed: true),
      AndroidBackOwner.system,
    );
    expect(host.pages, hasLength(1));
    expect(host.committed, isEmpty);
  });

  for (final kind in ['drawer', 'page', 'sheet']) {
    test('stale $kind target cannot dismiss replacement', () {
      final host = _BackHost();
      void replace(_Target target) {
        if (kind == 'drawer') {
          host.drawer = target;
        } else if (kind == 'page') {
          host.pages
            ..clear()
            ..add(target);
        } else {
          host.tokens
            ..clear()
            ..add(target);
        }
      }

      replace(_Target(kind));
      final attempt = host.coordinator.begin(imeVisible: false);
      final replacement = _Target(kind);
      replace(replacement);
      host.coordinator.finish(attempt, committed: true);
      expect(host.target, same(replacement));
      expect(host.committed, isEmpty);
    });
  }

  test('a new overlay does not let captured page pop beneath it', () {
    final host = _BackHost()..pages.add(_Target('page'));
    final attempt = host.coordinator.begin(imeVisible: false);
    host.tokens.add(_Target('confirmation'));
    host.coordinator.finish(attempt, committed: true);
    expect(host.pages, hasLength(1));
    expect(host.tokens, hasLength(1));
  });

  test('dirty form holds, cancel is inert, commit asks without discarding', () {
    final host = _BackHost()
      ..pages.add(_Target('page'))
      ..tokens.add(_Target('dirty form', canClose: false));
    final cancel = host.coordinator.begin(imeVisible: false);
    expect(cancel.canClose, isFalse);
    host.coordinator.finish(cancel, committed: false);
    expect(host.confirmations, 0);
    host.back();
    expect(host.confirmations, 1);
    expect(host.tokens.single.name, 'dirty form');
    expect(host.pages, hasLength(1));
    expect(host.draft, 'Unsent 日本語 👩🏽‍💻');
  });

  test('one attempt cannot commit twice or on a different coordinator', () {
    final host = _BackHost()..pages.add(_Target('page'));
    final attempt = host.coordinator.begin(imeVisible: false);
    expect(
      () => _BackHost().coordinator.finish(attempt, committed: true),
      throwsStateError,
    );
    host.coordinator.finish(attempt, committed: true);
    expect(
      () => host.coordinator.finish(attempt, committed: true),
      throwsStateError,
    );
    expect(host.committed, ['page']);
  });
}
