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
