import 'dart:async';

import 'package:factory_app/android/background_policy.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

class _Busy extends ValueNotifier<bool> {
  _Busy() : super(false);
  var listeners = 0;

  @override
  void addListener(VoidCallback listener) {
    listeners++;
    super.addListener(listener);
  }

  @override
  void removeListener(VoidCallback listener) {
    listeners--;
    super.removeListener(listener);
  }
}

void main() {
  late _Busy busy;
  late AndroidBackgroundPolicy policy;
  late int stops;

  setUp(() {
    busy = _Busy();
    stops = 0;
    policy = AndroidBackgroundPolicy(busy: busy, suspend: () => stops++);
  });

  tearDown(() {
    policy.dispose();
    busy.dispose();
  });

  testWidgets(
    'in front there is no stop, then hidden gets exactly 25 seconds',
    (tester) async {
      await tester.pump(const Duration(minutes: 10));
      expect(stops, 0);
      policy.hidden();
      await tester.pump(const Duration(milliseconds: 24999));
      expect(stops, 0);
      await tester.pump(const Duration(milliseconds: 1));
      expect(stops, 1);
      await tester.pump(const Duration(minutes: 10));
      expect(stops, 1);
    },
  );

  testWidgets('duplicate hidden edges never renew the grace period', (
    tester,
  ) async {
    policy.hidden();
    await tester.pump(const Duration(seconds: 24));
    policy.hidden();
    await tester.pump(const Duration(seconds: 1));
    expect(stops, 1);
  });

  testWidgets(
    'a 10-second trip keeps its connection; next trip gets fresh grace',
    (tester) async {
      policy.hidden();
      await tester.pump(const Duration(seconds: 10));
      policy.resumed();
      await tester.pump(const Duration(minutes: 10));
      expect(stops, 0);
      policy.hidden();
      await tester.pump(const Duration(seconds: 24));
      expect(stops, 0);
      await tester.pump(const Duration(seconds: 1));
      expect(stops, 1);
    },
  );

  testWidgets(
    'a whole upload or send may finish after grace, then stops at once',
    (tester) async {
      policy.hidden();
      busy.value = true;
      await tester.pump(const Duration(minutes: 10));
      expect(stops, 0, reason: 'no timeout cuts off a send still on its way');
      busy.value = false;
      await tester.pump(Duration.zero);
      expect(stops, 1, reason: 'finishing does not grant another 25 seconds');
      busy.value = true;
      busy.value = false;
      await tester.pump(Duration.zero);
      expect(stops, 1, reason: 'a hidden connection is suspended only once');
    },
  );

  testWidgets('work completing before grace does not shorten it', (
    tester,
  ) async {
    policy.hidden();
    busy.value = true;
    await tester.pump(const Duration(seconds: 10));
    busy.value = false;
    await tester.pump(const Duration(seconds: 14));
    expect(stops, 0);
    await tester.pump(const Duration(seconds: 1));
    expect(stops, 1);
  });

  testWidgets('resume cancels a queued stop after a protected send ends', (
    tester,
  ) async {
    busy.value = true;
    policy.hidden();
    await tester.pump(const Duration(seconds: 25));
    busy.value = false;
    policy.resumed();
    await tester.pump(Duration.zero);
    expect(stops, 0);
  });

  testWidgets('recording to upload handoff settles before an expired stop', (
    tester,
  ) async {
    busy.value = true;
    policy.hidden();
    await tester.pump(const Duration(seconds: 25));
    busy.value = false;
    scheduleMicrotask(() => busy.value = true);
    await tester.pump(Duration.zero);
    expect(stops, 0);
    busy.value = false;
    await tester.pump(Duration.zero);
    expect(stops, 1);
  });

  testWidgets(
    'dispose cancels grace and work listeners, late calls do nothing',
    (tester) async {
      expect(busy.listeners, 1);
      policy.hidden();
      policy.dispose();
      expect(busy.listeners, 0);
      policy.dispose();
      policy.hidden();
      busy.value = true;
      busy.value = false;
      await tester.pump(const Duration(minutes: 10));
      expect(stops, 0);
    },
  );

  testWidgets('each Claktory waits only for its own protected work', (
    tester,
  ) async {
    final otherBusy = ValueNotifier(false);
    var otherStops = 0;
    final other = AndroidBackgroundPolicy(
      busy: otherBusy,
      suspend: () => otherStops++,
    );
    busy.value = true;
    policy.hidden();
    other.hidden();
    await tester.pump(const Duration(seconds: 25));
    expect(stops, 0);
    expect(otherStops, 1);
    busy.value = false;
    await tester.pump(Duration.zero);
    expect(stops, 1);
    other.dispose();
    otherBusy.dispose();
  });

  testWidgets('visible Custom Tab protects only its own policy beyond grace', (
    tester,
  ) async {
    final other = AndroidBackgroundPolicy(busy: busy, suspend: () => stops++);
    policy.externalVisible(true);
    policy.hidden();
    other.hidden();
    await tester.pump(const Duration(minutes: 5));
    expect(stops, 1, reason: 'the unrelated Claktory still suspends');
    busy.value = true;
    busy.value = false;
    await tester.pump(Duration.zero);
    expect(stops, 1, reason: 'work finishing cannot close the visible tab');
    other.dispose();
  });

  testWidgets(
    'tab closing starts a fresh full grace, duplicates do not renew',
    (tester) async {
      policy.hidden();
      await tester.pump(const Duration(seconds: 24));
      policy.externalVisible(true);
      await tester.pump(const Duration(minutes: 5));
      expect(stops, 0);
      policy.externalVisible(false);
      await tester.pump(const Duration(seconds: 24));
      policy.externalVisible(false);
      policy.hidden();
      await tester.pump(const Duration(milliseconds: 999));
      expect(stops, 0);
      await tester.pump(const Duration(milliseconds: 1));
      expect(stops, 1);
    },
  );

  testWidgets('an initial false native event does not renew ordinary grace', (
    tester,
  ) async {
    policy.hidden();
    await tester.pump(const Duration(seconds: 24));
    policy.externalVisible(false);
    await tester.pump(const Duration(seconds: 1));
    expect(stops, 1);
  });

  testWidgets('late native visibility rearms a previously suspended policy', (
    tester,
  ) async {
    policy.hidden();
    await tester.pump(const Duration(seconds: 25));
    expect(stops, 1);
    policy.externalVisible(true);
    await tester.pump(const Duration(minutes: 5));
    expect(stops, 1);
    policy.externalVisible(false);
    await tester.pump(const Duration(seconds: 25));
    expect(stops, 2, reason: 'the transport can suspend again after reopening');
  });

  testWidgets('native visibility cancels a stop queued after work completes', (
    tester,
  ) async {
    busy.value = true;
    policy.hidden();
    await tester.pump(const Duration(seconds: 25));
    busy.value = false;
    policy.externalVisible(true);
    await tester.pump(Duration.zero);
    expect(stops, 0);
    policy.externalVisible(false);
    await tester.pump(const Duration(seconds: 25));
    expect(stops, 1);
  });

  testWidgets('returning to Flutter cancels a tab-close grace', (tester) async {
    policy.externalVisible(true);
    policy.hidden();
    policy.externalVisible(false);
    await tester.pump(const Duration(seconds: 24));
    policy.resumed();
    await tester.pump(const Duration(minutes: 5));
    expect(stops, 0);
    policy.hidden();
    await tester.pump(const Duration(seconds: 25));
    expect(stops, 1);
  });

  testWidgets(
    'tab close still waits for a send finishing after its new grace',
    (tester) async {
      policy.externalVisible(true);
      policy.hidden();
      policy.externalVisible(false);
      busy.value = true;
      await tester.pump(const Duration(seconds: 25));
      expect(stops, 0);
      busy.value = false;
      await tester.pump(Duration.zero);
      expect(stops, 1, reason: 'send completion does not grant another grace');
    },
  );

  testWidgets('disposed policy ignores late tab visibility', (tester) async {
    policy.hidden();
    policy.dispose();
    policy.externalVisible(true);
    policy.externalVisible(false);
    await tester.pump(const Duration(minutes: 5));
    expect(stops, 0);
  });

  testWidgets('a later background visit suspends again after resume', (
    tester,
  ) async {
    policy.hidden();
    await tester.pump(const Duration(seconds: 25));
    expect(stops, 1);
    policy.resumed();
    policy.hidden();
    await tester.pump(const Duration(seconds: 25));
    expect(stops, 2);
  });
}
