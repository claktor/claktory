import 'package:factory_app/android/share_browser_lifecycle.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'support/channel_fakes.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late ChannelEvents events;
  late AndroidShareBrowserLifecycle browser;
  late Object origin;
  late Object other;

  setUp(() {
    events = ChannelEvents('factory/share-browser/events');
    browser = AndroidShareBrowserLifecycle();
    origin = Object();
    other = Object();
  });

  tearDown(() async {
    browser.dispose();
    await Future<void>.delayed(Duration.zero);
    events.dispose();
  });

  test(
    'opening alone and an initial false event do not grant a hold',
    () async {
      final unwatch = browser.watch(() {});
      await Future<void>.delayed(Duration.zero);
      browser.opening(origin);
      expect(browser.visibleFor(origin), isFalse);
      await events.emit({'visible': false});
      expect(browser.visibleFor(origin), isFalse);
      await events.emit({'visible': true});
      expect(browser.visibleFor(origin), isTrue);
      expect(browser.visibleFor(other), isFalse);
      unwatch();
    },
  );

  test('initial native true without a known origin protects nobody', () async {
    browser.watch(() {});
    await Future<void>.delayed(Duration.zero);
    await events.emit({'visible': true});
    expect(browser.visibleFor(origin), isFalse);
    expect(browser.visibleFor(other), isFalse);
    browser.opening(origin);
    expect(browser.visibleFor(origin), isTrue);
  });

  test(
    'only real visibility edges notify, including close and rebind',
    () async {
      var changes = 0;
      browser.watch(() => changes++);
      await Future<void>.delayed(Duration.zero);
      browser.opening(origin);
      final registered = changes;
      await events.emit({'visible': false});
      expect(changes, registered);
      await events.emit({'visible': true});
      await events.emit({'visible': true});
      expect(changes, registered + 1);
      await events.emit({'visible': false});
      expect(changes, registered + 2);
      expect(browser.visibleFor(origin), isFalse);
      // Native process state survives the MainActivity being stopped.
      await events.emit({'visible': true});
      expect(browser.visibleFor(origin), isTrue);
      expect(browser.visibleFor(other), isFalse);
    },
  );

  test(
    'replacement origin and same-origin tokens ignore old open failure',
    () async {
      browser.watch(() {});
      await Future<void>.delayed(Duration.zero);
      final first = browser.opening(origin);
      await events.emit({'visible': true});
      final second = browser.opening(other);
      browser.openFailed(first);
      expect(browser.visibleFor(origin), isFalse);
      expect(browser.visibleFor(other), isTrue);
      final third = browser.opening(other);
      browser.openFailed(second);
      expect(browser.visibleFor(other), isTrue);
      browser.openFailed(third);
      expect(browser.visibleFor(other), isFalse);
      await events.emit({'visible': true});
      expect(browser.visibleFor(other), isFalse);
    },
  );

  test('ordinary browser fallback or failed open cannot keep a hold', () async {
    browser.watch(() {});
    await Future<void>.delayed(Duration.zero);
    final request = browser.opening(origin);
    browser.openFailed(request);
    await events.emit({'visible': true});
    expect(browser.visibleFor(origin), isFalse);
  });

  test('malformed events and native errors fail closed', () async {
    browser.watch(() {});
    await Future<void>.delayed(Duration.zero);
    browser.opening(origin);
    await events.emit({'visible': true});
    for (final invalid in [
      null,
      'true',
      {'visible': 'true'},
      {},
    ]) {
      await events.emit(invalid);
      expect(browser.visibleFor(origin), isFalse);
      await events.emit({'visible': true});
      expect(browser.visibleFor(origin), isTrue);
    }
    await events.error('failed', 'Fixture native error');
    expect(browser.visibleFor(origin), isFalse);
    await events.emit({'visible': true});
    expect(
      browser.visibleFor(origin),
      isFalse,
      reason: 'error forgot its origin',
    );
  });

  test(
    'one native subscription is shared and final cancellation forgets origin',
    () async {
      final first = browser.watch(() {});
      final second = browser.watch(() {});
      await Future<void>.delayed(Duration.zero);
      expect(events.listening, isTrue);
      browser.opening(origin);
      await events.emit({'visible': true});
      first();
      first();
      await Future<void>.delayed(Duration.zero);
      expect(events.listening, isTrue);
      expect(browser.visibleFor(origin), isTrue);
      second();
      await Future<void>.delayed(Duration.zero);
      expect(events.listening, isFalse);
      expect(browser.visibleFor(origin), isFalse);
      browser.watch(() {});
      await Future<void>.delayed(Duration.zero);
      await events.emit({'visible': true});
      expect(
        browser.visibleFor(origin),
        isFalse,
        reason: 'old origin is not retained',
      );
    },
  );

  test('removing an origin forgets it while other Claktories watch', () async {
    browser.watch(() {});
    browser.watch(() {});
    await Future<void>.delayed(Duration.zero);
    browser.opening(origin);
    await events.emit({'visible': true});
    browser.forget(other);
    expect(browser.visibleFor(origin), isTrue);
    browser.forget(origin);
    expect(browser.visibleFor(origin), isFalse);
    await events.emit({'visible': true});
    expect(browser.visibleFor(origin), isFalse);
  });

  test(
    'dispose cancels once and late callbacks or opens cannot revive it',
    () async {
      final unwatch = browser.watch(() {});
      await Future<void>.delayed(Duration.zero);
      final request = browser.opening(origin);
      await events.emit({'visible': true});
      browser.dispose();
      browser.dispose();
      unwatch();
      browser.openFailed(request);
      browser.opening(other);
      browser.watch(() {})();
      await Future<void>.delayed(Duration.zero);
      expect(events.listening, isFalse);
      expect(browser.visibleFor(origin), isFalse);
      expect(browser.visibleFor(other), isFalse);
    },
  );

  test(
    'public Android helpers register and release the originating Backend',
    () async {
      debugDefaultTargetPlatformOverride = TargetPlatform.android;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final backend = RecordingBackend();
      final unwatch = androidShareBrowserLifecycle.watch(() {});
      addTearDown(() async {
        unwatch();
        await backend.close();
      });
      await Future<void>.delayed(Duration.zero);
      final request = shareBrowserOpening(backend);
      await events.emit({'visible': true});
      expect(androidShareBrowserLifecycle.visibleFor(backend), isTrue);
      shareBrowserOpenFailed(request);
      expect(androidShareBrowserLifecycle.visibleFor(backend), isFalse);
    },
  );

  test('public opening helpers are no-ops off Android', () async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final backend = RecordingBackend();
    addTearDown(backend.close);
    final request = shareBrowserOpening(backend);
    shareBrowserOpenFailed(request);
    expect(androidShareBrowserLifecycle.visibleFor(backend), isFalse);
    expect(events.listening, isFalse);
  });
}
