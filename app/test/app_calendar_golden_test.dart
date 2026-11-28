// Golden screenshots of the catalog's Calendar (Lucy's ux/calendar.md) at a
// Mac's, an iPad's and a phone's widths: a week, its details open; a
// phone's agenda, its day and an entry's sheet. Thursday 3 December 2026,
// 14:20, in Geist as the app ships it.
//
// Update them after an intended change of look:
//   flutter test --update-goldens test/app_calendar_golden_test.dart
import 'dart:convert';

import 'package:factory_app/app_views/app_view.dart';
import 'package:factory_app/app_views/calendar.dart';
import 'package:factory_app/app_views/transport.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/open_place.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_calendar_test.dart'
    show
        calendarColumns,
        calendarNow,
        calendarRows,
        calendarTools,
        calendarView,
        calendarWorkspace;
import 'app_views_test.dart' show FakeAppViewTransport;
import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

const _canvas = Key('canvas');

/// The Mac's offset: six hours behind this device, so the bar says so.
final _mac = calendarNow.timeZoneOffset.inMinutes - 360;

Future<void> _show(
  WidgetTester tester,
  Map<String, Object?> calendar, {
  required Size size,
  Brightness brightness = Brightness.dark,
}) async {
  tester.platformDispatcher.platformBrightnessTestValue = brightness;
  addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
  final snapshot = jsonEncode({
    'version': '1.0',
    'createSurface': {
      'surfaceId': 'calendar-1',
      'catalogId': appViewCatalogV1,
      'components': [
        {
          'id': 'root',
          'component': 'Column',
          'children': ['cal'],
        },
        {
          ...calendar,
          'height': size.height - 80,
          'tools': ['agent_menu', 'show_menu', 'more_menu'],
          'macZone': {'start': _mac, 'end': _mac},
        },
        ...calendarTools,
      ],
      'dataModel': {
        'rows': {'columns': calendarColumns, 'rows': calendarRows},
      },
    },
  });
  final transport = FakeAppViewTransport()
    ..answer = (_) => AppSurfaceData(
      surfaceId: 'calendar-1',
      appId: 'calendar',
      title: 'Calendar',
      state: AppViewState.open,
      snapshot: snapshot,
      seq: 0,
      error: '',
    );
  await Harness(calendarWorkspace).show(
    tester,
    size: size,
    home: Builder(
      builder: (context) => ColoredBox(
        key: _canvas,
        color: Palette.of(context).canvas,
        child: OpenPlace(
          gone: (_) async => null,
          open: (_, _) {},
          child: AppView(
            transport: transport,
            appId: 'calendar',
            appName: 'Calendar',
          ),
        ),
      ),
    ),
  );
  await tester.pump();
  await tester.pump(Motion.base);
  await tester.pump(const Duration(seconds: 1));
}

Future<void> _golden(WidgetTester tester, String name, Brightness shade) =>
    expectLater(
      find.byKey(_canvas),
      matchesGoldenFile('goldens/calendar_${name}_${shade.name}.png'),
    );

void main() {
  setUpAll(() async {
    await loadGeist();
    WidgetsApp.debugAllowBannerOverride = false;
  });
  setUp(() => AppCalendar.clock = () => calendarNow);
  tearDown(() => AppCalendar.clock = DateTime.now);

  for (final shade in Brightness.values) {
    testWidgets('${shade.name}: a week on a Mac', (tester) async {
      await _show(
        tester,
        calendarView(),
        size: const Size(1280, 860),
        brightness: shade,
      );
      expect(tester.takeException(), isNull);
      await _golden(tester, 'mac_week', shade);
    });
  }

  testWidgets("the bar at an iPad's widths: the heading never touches the "
      "app's tools; where they don't fit, they go under it", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    for (var width = 700.0; width <= 1300; width += 6) {
      for (final view in ['day', 'week']) {
        await tester.pumpWidget(const SizedBox());
        await _show(tester, calendarView(view: view), size: Size(width, 1000));
        final heading = tester.getRect(
          find.text(
            view == 'day' ? 'Thursday 3 December' : '30 Nov – 6 Dec 2026',
          ),
        );
        final agent = tester.getRect(find.text('Agent: All'));
        final sameLine =
            agent.top < heading.bottom && heading.top < agent.bottom;
        if (sameLine) {
          expect(
            agent.left - heading.right,
            greaterThanOrEqualTo(Space.l),
            reason: '$view at $width',
          );
        }
      }
    }
  });

  testWidgets("a week on a Mac, a monitor's details open beside it", (
    tester,
  ) async {
    await _show(tester, calendarView(), size: const Size(1280, 860));
    await tester.tap(
      find.textContaining('Production logs', findRichText: true).last,
    );
    await tester.pumpAndSettle();
    await _golden(tester, 'mac_details', Brightness.dark);
  });

  testWidgets('a week on an iPad in portrait', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    await _show(tester, calendarView(), size: const Size(820, 1180));
    expect(tester.takeException(), isNull);
    await _golden(tester, 'ipad_week', Brightness.dark);
  });

  for (final (view, name) in [('week', 'agenda'), ('day', 'day')]) {
    testWidgets("a phone's $name", (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      await _show(tester, calendarView(view: view), size: const Size(390, 844));
      expect(tester.takeException(), isNull);
      await _golden(tester, 'phone_$name', Brightness.dark);
    });
  }

  testWidgets("a phone's day, an item's sheet up", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    await _show(tester, calendarView(view: 'day'), size: const Size(390, 844));
    await tester.tap(
      find.textContaining('#18 History video', findRichText: true),
    );
    await tester.pumpAndSettle();
    await _golden(tester, 'phone_sheet', Brightness.dark);
  });
}
