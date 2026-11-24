// Settings → Notifications (#273 B4): which kinds notify, how much a
// notification shows, quiet hours, whether this device gets them (the
// system's permission, read again when the app comes back), a test
// notification; and an agent's "Notify me". Each change sends the settings
// with only that change made.

import 'package:factory_app/forms.dart' show AgentSettingsForm;
import 'package:factory_app/notification_settings.dart';
import 'package:factory_app/controls.dart' show Button, showModal;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/diagnostics_record.dart' show PushStep;
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'idle_compaction_test.dart' show reveal;

NotificationSettings _settings() => NotificationSettings(
  needsYou: true,
  alerts: true,
  replies: true,
  preview: NotifyPreview.NOTIFY_PREVIEW_FULL,
  quietStart: 22 * 60,
  quietEnd: 7 * 60,
);

class _FakeDevice implements DeviceNotifications {
  _FakeDevice(this.state, {this.pushes = false, this.name = 'This Mac'});

  String state;
  var asked = 0;
  var opened = 0;

  /// What the person answers when asked.
  String answer = 'allowed';

  @override
  final bool pushes;

  @override
  final String name;

  @override
  Future<String> permission() async => state;

  @override
  Future<String> ask() async {
    asked++;
    if (state == 'not_determined') state = answer;
    return state;
  }

  @override
  Future<void> openSettings() async => opened++;

  @override
  final ValueNotifier<Map<Backend, PushStep>> setupFailures = ValueNotifier(
    const {},
  );
  final retried = <Backend>[];

  @override
  void retrySetup(Backend backend) => retried.add(backend);
}

Future<Harness> _show(
  WidgetTester tester, {
  NotificationSettings? settings,
  bool has = true,
  _FakeDevice? device,
  bool noDevice = false,
  RemoteAccess? remote,
  Size size = const Size(1200, 2000),
}) async {
  DeviceNotifications.debugOverride = noDevice
      ? null
      : device ?? _FakeDevice('allowed');
  addTearDown(() => DeviceNotifications.debugOverride = null);
  final snapshot = workspace();
  if (has) snapshot.settings.notifications = settings ?? _settings();
  if (remote != null) snapshot.remote = remote;
  final harness = Harness(snapshot);
  await harness.show(tester, size: size);
  harness.page.value = AppPage.settings;
  await settle(tester);
  return harness;
}

/// Taps [target] once it's in view.
Future<void> _tap(WidgetTester tester, Finder target) async {
  await tester.ensureVisible(target);
  await pumpFrames(tester);
  await tester.tap(target);
  await settle(tester);
  // What it set off (a page's first read) shows in the next frames.
  await pumpFrames(tester);
}

UpdateSettings? _lastUpdate(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasUpdateSettings()) message.updateSettings,
].lastOrNull;

void main() {
  test('quiet hours on either clock', () {
    expect(minutesOfDay(22 * 60), '22:00');
    expect(minutesOfDay(7 * 60 + 30), '07:30');
    expect(minutesOfDay(0, h24: false), '12:00 AM');
    expect(minutesOfDay(22 * 60, h24: false), '10:00 PM');
    expect(minutesOfDay(12 * 60, h24: false), '12:00 PM');
  });

  test("quiet hours' time zone, named when it isn't this device's", () {
    final now = DateTime.now();
    final here = now.timeZoneOffset.inMinutes;
    expect(
      quietHoursNote(NotificationSettings(), now),
      'Notifications wait until these hours end, in the time zone of the '
      'computer Claktory runs on.',
    );
    expect(
      quietHoursNote(
        NotificationSettings(timeZone: 'Europe/Rome', utcOffsetMinutes: here),
        now,
      ),
      'Notifications wait until these hours end.',
    );
    expect(
      quietHoursNote(
        NotificationSettings(
          timeZone: 'America/New_York',
          utcOffsetMinutes: here + 60,
        ),
        now,
      ),
      'Notifications wait until these hours end, in America/New York time.',
    );
  });

  test("the phone list's summary", () {
    expect(notificationsSummary(_settings()), '');
    expect(notificationsSummary(_settings()..quietHours = true), 'Quiet hours');
    expect(
      notificationsSummary(
        NotificationSettings(needsYou: false, alerts: false, replies: false),
      ),
      'Off',
    );
  });

  testWidgets('a switch sends the settings with only it changed', (
    tester,
  ) async {
    final harness = await _show(tester);
    expect(find.text('Notifications'), findsWidgets);
    await reveal(tester, find.text('An agent alerts you'));
    await _tap(tester, find.text('An agent alerts you'));
    expect(
      _lastUpdate(harness),
      UpdateSettings(
        notificationChange: NotificationSettingsChange(alerts: false),
      ),
    );

    // Replies from a computer follow replies.
    await reveal(
      tester,
      find.text('Also replies to messages sent from a computer'),
    );
    await _tap(
      tester,
      find.text('Also replies to messages sent from a computer'),
    );
    expect(
      _lastUpdate(harness),
      UpdateSettings(
        notificationChange: NotificationSettingsChange(desktopReplies: true),
      ),
    );
    await harness.apply(
      tester,
      workspace()..settings.notifications = (_settings()..replies = false),
    );
    final sent = harness.backend.sent.length;
    await _tap(
      tester,
      find.text('Also replies to messages sent from a computer'),
    );
    expect(harness.backend.sent.length, sent, reason: 'off with replies');

    harness.backend.refuse = 'Quiet hours run 0 to 1439 minutes.';
    await _tap(tester, find.text('An agent needs you'));
    expect(find.text('Quiet hours run 0 to 1439 minutes.'), findsOneWidget);
  });

  testWidgets('how much a notification shows', (tester) async {
    final harness = await _show(tester);
    await reveal(tester, find.text('Who and what it says'));
    expect(
      find.text(
        'If an agent has read a web page or answers a pipe, what it says '
        'stays in the app.',
      ),
      findsOneWidget,
    );
    await _tap(tester, find.text('Who and what it says'));
    await _tap(tester, find.text('Nothing').last);
    expect(
      _lastUpdate(harness),
      UpdateSettings(
        notificationChange: NotificationSettingsChange(
          preview: NotifyPreview.NOTIFY_PREVIEW_NONE,
        ),
      ),
    );
    await harness.apply(
      tester,
      workspace()
        ..settings.notifications = (_settings()
          ..preview = NotifyPreview.NOTIFY_PREVIEW_NONE),
    );
    expect(
      find.text('Shows “Claktory has something for you.”'),
      findsOneWidget,
    );
  });

  testWidgets('quiet hours: on, then their times and urgent alerts', (
    tester,
  ) async {
    tester.platformDispatcher.alwaysUse24HourFormatTestValue = true;
    addTearDown(tester.platformDispatcher.clearAlwaysUse24HourTestValue);
    final harness = await _show(tester);
    await reveal(
      tester,
      find.text(
        'Notifications wait until these hours end, in the time zone of the '
        'computer Claktory runs on.',
      ),
    );
    expect(find.text('From'), findsNothing, reason: 'off: no times');
    await _tap(tester, find.text('Quiet hours').last);
    expect(
      _lastUpdate(harness),
      UpdateSettings(
        notificationChange: NotificationSettingsChange(quietHours: true),
      ),
    );
    await harness.apply(
      tester,
      workspace()..settings.notifications = (_settings()..quietHours = true),
    );
    await reveal(tester, find.text('Let urgent alerts through'));
    expect(find.text('22:00'), findsOneWidget);
    expect(find.text('07:00'), findsOneWidget);
    await _tap(tester, find.text('22:00'));
    await _tap(tester, find.text('21:00').last);
    expect(
      _lastUpdate(harness),
      UpdateSettings(
        notificationChange: NotificationSettingsChange(quietStart: 21 * 60),
      ),
    );
    await _tap(tester, find.text('Let urgent alerts through'));
    expect(
      _lastUpdate(harness),
      UpdateSettings(
        notificationChange: NotificationSettingsChange(urgentBreaksQuiet: true),
      ),
    );
  });

  testWidgets("a time's menu opens at the time chosen", (tester) async {
    tester.platformDispatcher.alwaysUse24HourFormatTestValue = true;
    addTearDown(tester.platformDispatcher.clearAlwaysUse24HourTestValue);
    await _show(
      tester,
      settings: _settings()
        ..quietHours = true
        ..quietStart = 21 * 60 + 30,
    );
    await reveal(tester, find.text('Let urgent alerts through'));
    expect(find.text('21:30'), findsOneWidget);
    await _tap(tester, find.text('21:30'));
    expect(
      find.text('21:30'),
      findsNWidgets(2),
      reason: 'the field, and its row in the menu, shown',
    );
    final row = tester.getRect(find.text('21:30').last);
    final menu = tester.getRect(
      find
          .ancestor(
            of: find.text('21:30').last,
            matching: find.byType(ListView),
          )
          .first,
    );
    expect(menu.contains(row.center), isTrue);
  });

  testWidgets('a test notification', (tester) async {
    final harness = await _show(tester);
    await reveal(tester, find.text('Send a test notification'));
    expect(
      find.text(
        'It goes to every device that gets notifications, even in quiet '
        'hours.',
      ),
      findsOneWidget,
    );
    await _tap(tester, find.text('Send a test notification'));
    expect(harness.last.hasSendTestNotification(), isTrue);
    // Where it went: this device, as it shows notifications.
    expect(find.text('Sent to this Mac.'), findsOneWidget);
  });

  test('where a test notification went (#352 P2)', () {
    PairedDevice paired(
      String id,
      String name, {
      String platform = '',
      bool connected = false,
      bool notifications = false,
    }) => PairedDevice(
      id: id,
      name: name,
      platform: platform,
      connected: connected,
      notifications: notifications,
    );
    final devices = [
      // Registered for pushes: gets it, connected or not.
      paired('d1', "Matt's iPhone", platform: 'ios', notifications: true),
      // A Mac with Claktory open shows it; closed, it doesn't.
      paired('d2', 'Studio', platform: 'macos', connected: true),
      paired('d3', 'Laptop', platform: 'macos'),
      // Android shows none yet, connected or not.
      paired('d4', 'Pixel', platform: 'android', connected: true),
      // An iPad that hasn't turned them on, and an app too old to say.
      paired('d5', "Matt's iPad", platform: 'ipados', connected: true),
      paired('d6', 'Old', connected: true),
    ];
    expect(
      testSentTo(devices: devices, thisDevice: 'local', here: 'this Mac'),
      "Sent to this Mac, Matt's iPhone and Studio.",
    );
    // This device is named once, as "this …", never again by its name.
    expect(
      testSentTo(devices: devices, thisDevice: 'd1', here: 'this iPhone'),
      'Sent to this iPhone and Studio.',
    );
    expect(
      testSentTo(devices: devices.take(1), thisDevice: 'd4'),
      "Sent to Matt's iPhone.",
    );
    expect(
      testSentTo(devices: devices.skip(2), thisDevice: 'd4'),
      'No device gets notifications yet. Turn them on in Claktory on an '
      'iPhone, iPad or Mac.',
    );
  });

  testWidgets("Android: this phone says it doesn't get notifications yet, "
      'and the test names where it went', (tester) async {
    await _show(
      tester,
      noDevice: true,
      remote: RemoteAccess(
        devices: [
          PairedDevice(id: 'me', name: 'Pixel', platform: 'android'),
          PairedDevice(
            id: 'd1',
            name: "Matt's iPhone",
            platform: 'ios',
            notifications: true,
          ),
        ],
      ),
    );
    await reveal(tester, find.text('Send a test notification'));
    expect(
      find.textContaining("doesn't get notifications yet."),
      findsOneWidget,
    );
    expect(find.text('Turn on'), findsNothing);
    await _tap(tester, find.text('Send a test notification'));
    expect(find.text("Sent to Matt's iPhone."), findsOneWidget);
    expect(find.text('Sent.'), findsNothing);
  });

  testWidgets('a backend without them: no Notifications section', (
    tester,
  ) async {
    await _show(tester, has: false);
    expect(find.text('An agent needs you'), findsNothing);
  });

  group('this device', () {
    testWidgets('a Mac: allowed, not asked yet, turned off', (tester) async {
      final device = _FakeDevice('allowed');
      await _show(tester, device: device);
      await reveal(tester, find.text('This Mac'));
      expect(
        find.text('Shows notifications while Claktory is open on it.'),
        findsOneWidget,
      );
      expect(find.text('Turn on'), findsNothing);
    });

    testWidgets('not asked yet: Turn on asks', (tester) async {
      final device = _FakeDevice('not_determined');
      await _show(tester, device: device);
      await reveal(tester, find.text('Turn on'));
      expect(find.text('Not turned on yet.'), findsOneWidget);
      await _tap(tester, find.text('Turn on'));
      expect(device.asked, 1);
      expect(
        find.text('Shows notifications while Claktory is open on it.'),
        findsOneWidget,
      );
    });

    testWidgets('an iPhone turned off in Settings: Open Settings, and back '
        'in the app it reads again', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final device = _FakeDevice('denied', pushes: true, name: 'This iPhone');
      final snapshot = workspace()
        ..settings.notifications = _settings()
        ..remote = RemoteAccess(
          devices: [PairedDevice(id: 'phone', notifications: true)],
        );
      DeviceNotifications.debugOverride = device;
      addTearDown(() => DeviceNotifications.debugOverride = null);
      final harness = Harness(snapshot);
      harness.backend.deviceId = 'phone';
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      await harness.show(tester, size: const Size(393, 852));
      harness.page.value = AppPage.settings;
      await pumpFrames(tester);
      await _tap(tester, find.text('Notifications'));
      final open = find.text('Open Settings');
      await tester.ensureVisible(open);
      await pumpFrames(tester);
      expect(find.text('This iPhone'), findsOneWidget);
      expect(find.text('Turned off for Claktory in Settings.'), findsOneWidget);
      await _tap(tester, open);
      expect(device.opened, 1);
      // Turned on there; back in the app.
      device.state = 'allowed';
      for (final state in [
        AppLifecycleState.inactive,
        AppLifecycleState.hidden,
        AppLifecycleState.paused,
        AppLifecycleState.hidden,
        AppLifecycleState.inactive,
        AppLifecycleState.resumed,
      ]) {
        tester.binding.handleAppLifecycleStateChanged(state);
      }
      await pumpFrames(tester);
      expect(
        find.text('Gets notifications, even when Claktory is closed.'),
        findsOneWidget,
      );
      // Not registered with this Claktory (yet).
      await harness.apply(
        tester,
        workspace()
          ..settings.notifications = _settings()
          ..remote = RemoteAccess(devices: [PairedDevice(id: 'phone')]),
      );
      expect(
        find.text("Gets notifications once it's connected to this Claktory."),
        findsOneWidget,
      );
    });

    testWidgets("an iPhone that couldn't set up its notifications with this "
        'Claktory says so, with Try again, until it can', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final device = _FakeDevice('allowed', pushes: true, name: 'This iPhone');
      final snapshot = workspace()
        ..settings.notifications = _settings()
        ..remote = RemoteAccess(devices: [PairedDevice(id: 'phone')]);
      DeviceNotifications.debugOverride = device;
      addTearDown(() => DeviceNotifications.debugOverride = null);
      final harness = Harness(snapshot);
      harness.backend.deviceId = 'phone';
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      await harness.show(tester, size: const Size(393, 852));
      harness.page.value = AppPage.settings;
      await pumpFrames(tester);
      await _tap(tester, find.text('Notifications'));
      expect(
        find.text("Gets notifications once it's connected to this Claktory."),
        findsOneWidget,
      );

      // v0.1.16: the keychain refused its key.
      device.setupFailures.value = {harness.backend: PushStep.key};
      await pumpFrames(tester);
      expect(
        find.text(
          "Couldn't set up notifications. Claktory tries again each time it "
          'connects.',
        ),
        findsOneWidget,
      );
      final again = find.text('Try again');
      await _tap(tester, again);
      expect(device.retried, [harness.backend]);
      expect(
        tester
            .getSize(
              find.ancestor(of: again, matching: find.byType(Button)).first,
            )
            .height,
        greaterThanOrEqualTo(44),
      );

      // It worked.
      device.setupFailures.value = const {};
      await harness.apply(
        tester,
        workspace()
          ..settings.notifications = _settings()
          ..remote = RemoteAccess(
            devices: [PairedDevice(id: 'phone', notifications: true)],
          ),
      );
      expect(find.text('Try again'), findsNothing);
      expect(
        find.text('Gets notifications, even when Claktory is closed.'),
        findsOneWidget,
      );
    });

    testWidgets('an iPhone not asked yet: Turn on, a finger\'s size, at the '
        "line's end", (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final device = _FakeDevice(
        'not_determined',
        pushes: true,
        name: 'This iPhone',
      );
      DeviceNotifications.debugOverride = device;
      addTearDown(() => DeviceNotifications.debugOverride = null);
      final harness = Harness(
        workspace()..settings.notifications = _settings(),
      );
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      await harness.show(tester, size: const Size(393, 852));
      harness.page.value = AppPage.settings;
      await pumpFrames(tester);
      expect(
        find.text('?'),
        findsNothing,
        reason: "every row's glyph is drawn: a bell, not a fallback",
      );
      await _tap(tester, find.text('Notifications'));
      final turnOn = find.text('Turn on');
      await tester.ensureVisible(turnOn);
      await pumpFrames(tester);
      final button = tester.getRect(
        find.ancestor(of: turnOn, matching: find.byType(Container)).first,
      );
      expect(button.height, greaterThanOrEqualTo(44));
      expect(button.right, closeTo(393 - 16, 1));
      await _tap(tester, turnOn);
      expect(device.asked, 1);
    });
  });

  testWidgets("an agent's Notify me sends only it", (tester) async {
    final harness = await _show(tester);
    harness.page.value = AppPage.agent;
    await settle(tester);
    final agent = harness.backend.workspace.workspace.agents.first;
    showModal(
      tester.element(find.text(agent.name).first),
      (context, close) => AgentSettingsForm(agent: agent, close: close),
    );
    await settle(tester);
    final field = find.text('Notify me');
    await tester.ensureVisible(field);
    await settle(tester);
    await _tap(tester, find.text('Default'));
    await _tap(tester, find.text('Never').last);
    await _tap(tester, find.text('Save'));
    final update = harness.backend.sent.lastWhere((m) => m.hasUpdateAgent());
    expect(
      update.updateAgent,
      UpdateAgent(agentId: agent.id, notify: NotifyMode.NOTIFY_MODE_NEVER),
    );
  });
}
