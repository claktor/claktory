// Settings → Notifications (#273 B4, notifications.md §4 in Research): what
// notifies the person and how much it shows, quiet hours, whether this
// device gets them, and a test. All of it is the backend's
// (Settings.notifications), but this device's own permission, which is the
// system's.
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart' show Backend;
import 'controls.dart';
import 'devices.dart' show thisDeviceKind;
import 'diagnostics_record.dart' show PushStep;
import 'identity.dart';
import 'mobile/lines.dart' show ActionLine;
import 'mobile/metrics.dart';
import 'mobile/push.dart' show PushRegistrar;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'settings_view.dart' show SettingRequests, SettingRow, controlWidth;

/// What Settings' list says of Notifications on a phone: "Off" when no kind
/// notifies, "Quiet hours" while they're on, else nothing.
String notificationsSummary(NotificationSettings settings) {
  if (!settings.needsYou && !settings.alerts && !settings.replies) {
    return 'Off';
  }
  return settings.quietHours ? 'Quiet hours' : '';
}

/// How much a notification shows, as the person picks it.
const previewOptions = [
  SelectOption(NotifyPreview.NOTIFY_PREVIEW_FULL, 'Who and what it says'),
  SelectOption(NotifyPreview.NOTIFY_PREVIEW_WHO, 'Only who'),
  SelectOption(NotifyPreview.NOTIFY_PREVIEW_NONE, 'Nothing'),
];

/// Whether an agent notifies the person (its settings' "Notify me").
const notifyModeOptions = [
  SelectOption(NotifyMode.NOTIFY_MODE_DEFAULT, 'Default'),
  SelectOption(NotifyMode.NOTIFY_MODE_ALWAYS, 'Always'),
  SelectOption(NotifyMode.NOTIFY_MODE_NEVER, 'Never'),
];

/// Settings → Notifications. Each change sends the settings with only that
/// change; the Workspace that follows shows it.
class NotificationSettingsView extends StatefulWidget {
  const NotificationSettingsView({super.key, required this.settings});

  final NotificationSettings settings;

  @override
  State<NotificationSettingsView> createState() =>
      _NotificationSettingsViewState();
}

class _NotificationSettingsViewState extends State<NotificationSettingsView>
    with SettingRequests {
  /// Only what changed (NotificationSettingsChange): a change made on
  /// another device meanwhile stays. What shows is the Workspace's, so a
  /// backend that ignored it shows it as it was.
  void _save(NotificationSettingsChange change) => run(
    () =>
        AppScope.of(context)
            .updateSettings(UpdateSettings(notificationChange: change)),
  );

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final settings = widget.settings;
    final caption = type.caption.copyWith(color: palette.textFaint);
    final preview = settings.preview == NotifyPreview.NOTIFY_PREVIEW_UNSPECIFIED
        ? NotifyPreview.NOTIFY_PREVIEW_FULL
        : settings.preview;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.only(bottom: Space.m),
          child: Text(
            "Claktory lets you know when something happens while you're "
            "away. Nothing notifies while you're using Claktory on any "
            'device.',
            style: type.caption.copyWith(color: palette.textMuted),
          ),
        ),
        SettingRow(
          label: 'Notify me when',
          top: true,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              NotificationSwitch(
                label: 'An agent needs you',
                note:
                    'A question, a request for a secret, a proposal, or a '
                    'pipe message to review.',
                value: settings.needsYou,
                onChanged: (on) =>
                    _save(NotificationSettingsChange(needsYou: on)),
              ),
              const SizedBox(height: Space.s),
              NotificationSwitch(
                label: 'An agent alerts you',
                note: "When an agent decides something can't wait.",
                value: settings.alerts,
                onChanged: (on) =>
                    _save(NotificationSettingsChange(alerts: on)),
              ),
              const SizedBox(height: Space.s),
              NotificationSwitch(
                label: 'An agent replies to you',
                note: 'To a message you sent from your phone or tablet.',
                value: settings.replies,
                onChanged: (on) =>
                    _save(NotificationSettingsChange(replies: on)),
              ),
              const SizedBox(height: Space.s),
              Padding(
                padding: const EdgeInsets.only(left: Space.xl),
                child: NotificationSwitch(
                  label: 'Also replies to messages sent from a computer',
                  value: settings.desktopReplies,
                  onChanged: settings.replies
                      ? (on) => _save(
                          NotificationSettingsChange(desktopReplies: on),
                        )
                      : null,
                ),
              ),
            ],
          ),
        ),
        SettingRow(
          label: 'Show in notifications',
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(
                width: controlWidth(context, 240),
                child: Select(
                  title: 'Show in notifications',
                  value: preview,
                  options: previewOptions,
                  onChanged: (value) {
                    if (value != preview) {
                      _save(NotificationSettingsChange(preview: value));
                    }
                  },
                ),
              ),
              if (preview == NotifyPreview.NOTIFY_PREVIEW_NONE) ...[
                const SizedBox(height: Space.xs),
                Text('Shows “Claktory has something for you.”', style: caption),
              ],
              const SizedBox(height: Space.xs),
              Text(
                'If an agent has read a web page or answers a pipe, what it '
                'says stays in the app.',
                style: caption,
              ),
            ],
          ),
        ),
        SettingRow(
          label: 'Quiet hours',
          top: true,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              NotificationSwitch(
                label: 'Quiet hours',
                note: quietHoursNote(settings, DateTime.now()),
                value: settings.quietHours,
                onChanged: (on) =>
                    _save(NotificationSettingsChange(quietHours: on)),
              ),
              if (settings.quietHours) ...[
                const SizedBox(height: Space.s),
                Padding(
                  padding: const EdgeInsets.only(left: Space.xl),
                  child: _QuietTimes(
                    start: settings.quietStart,
                    end: settings.quietEnd,
                    onStart: (minutes) =>
                        _save(NotificationSettingsChange(quietStart: minutes)),
                    onEnd: (minutes) =>
                        _save(NotificationSettingsChange(quietEnd: minutes)),
                  ),
                ),
                const SizedBox(height: Space.s),
                Padding(
                  padding: const EdgeInsets.only(left: Space.xl),
                  child: NotificationSwitch(
                    label: 'Let urgent alerts through',
                    note: 'Alerts an agent marks urgent still notify.',
                    value: settings.urgentBreaksQuiet,
                    onChanged: (on) => _save(
                      NotificationSettingsChange(urgentBreaksQuiet: on),
                    ),
                  ),
                ),
              ],
            ],
          ),
        ),
        const ThisDeviceNotifications(),
        failureLine(palette),
      ],
    );
  }
}

/// Quiet hours' From and To, each a choice of half hours on this device's
/// clock (the time it's set to added when it isn't a half hour).
class _QuietTimes extends StatelessWidget {
  const _QuietTimes({
    required this.start,
    required this.end,
    required this.onStart,
    required this.onEnd,
  });

  final int start;
  final int end;
  final ValueChanged<int> onStart;
  final ValueChanged<int> onEnd;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final h24 = MediaQuery.maybeAlwaysUse24HourFormatOf(context) ?? true;
    Widget pick(String label, int value, ValueChanged<int> onChanged) => Wrap(
      spacing: Space.s,
      runSpacing: Space.xs,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        SizedBox(
          width: isPhoneLayout(context) ? null : 40,
          child: Text(
            label,
            style: TypeScale.of(context).body
                .copyWith(color: palette.textMuted),
          ),
        ),
        SizedBox(
          width: isPhoneLayout(context) ? 160 : 120,
          child: Select(
            title: label,
            value: value,
            options: [
              if (value % 30 != 0 || value >= 1440)
                SelectOption(value, minutesOfDay(value, h24: h24)),
              for (var minutes = 0; minutes < 1440; minutes += 30)
                SelectOption(minutes, minutesOfDay(minutes, h24: h24)),
            ],
            onChanged: (minutes) {
              if (minutes != value) onChanged(minutes);
            },
          ),
        ),
      ],
    );
    return Wrap(
      spacing: Space.l,
      runSpacing: Space.s,
      children: [pick('From', start, onStart), pick('To', end, onEnd)],
    );
  }
}

/// What quiet hours' times are in: the Claktory's time zone, named when
/// it isn't this device's own now ([now]'s offset), since the times are
/// that zone's.
String quietHoursNote(NotificationSettings settings, DateTime now) {
  // What comes meanwhile waits: once they end, one notification says how
  // much needs the person (#364 O3).
  const plain = 'Notifications wait until these hours end';
  if (settings.timeZone.isEmpty) {
    return '$plain, in the time zone of the computer Claktory runs on.';
  }
  if (settings.utcOffsetMinutes == now.timeZoneOffset.inMinutes) {
    return '$plain.';
  }
  return '$plain, in ${settings.timeZone.replaceAll('_', ' ')} time.';
}

/// [minutes] after midnight as a clock: "22:00", or on a 12-hour clock
/// ([h24] false) "10:00 PM".
String minutesOfDay(int minutes, {bool h24 = true}) {
  final hour = (minutes ~/ 60) % 24;
  final minute = (minutes % 60).toString().padLeft(2, '0');
  if (h24) return '${hour.toString().padLeft(2, '0')}:$minute';
  final twelve = hour % 12 == 0 ? 12 : hour % 12;
  return '$twelve:$minute ${hour < 12 ? 'AM' : 'PM'}';
}

/// A switch with its label and, when there is one, a note under it; the
/// whole row turns it over. As Settings' other switches.
class NotificationSwitch extends StatelessWidget {
  const NotificationSwitch({
    super.key,
    required this.label,
    this.note = '',
    required this.value,
    required this.onChanged,
  });

  final String label;
  final String note;
  final bool value;

  /// Null when it can't be changed now.
  final ValueChanged<bool>? onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final enabled = onChanged != null;
    return ToggleTap(
      value: value,
      onChanged: onChanged,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 2),
            child: Toggle(value: value, label: label, onChanged: onChanged),
          ),
          const SizedBox(width: Space.m),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  label,
                  style: TypeScale.of(context).body.copyWith(
                    color: enabled ? palette.text : palette.textFaint,
                  ),
                ),
                if (note.isNotEmpty) ...[
                  const SizedBox(height: 2),
                  Text(
                    note,
                    style: TypeScale.of(context).caption
                        .copyWith(color: palette.textMuted),
                  ),
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// What Send a test notification says once it's sent: the devices it
/// reached (#352 P2), [here] first ("this Mac", when this device shows
/// notifications; null when it doesn't), then the paired devices that show
/// it: an iPhone or iPad registered for pushes, a Mac connected now. An
/// Android device shows none yet, and one whose system isn't known (an
/// older app or backend) isn't claimed.
String testSentTo({
  required Iterable<PairedDevice> devices,
  required String thisDevice,
  String? here,
}) {
  final names = [
    ?here,
    for (final device in devices)
      if (device.id != thisDevice &&
          (device.notifications ||
              (device.connected && device.platform == 'macos')))
        device.name,
  ];
  return switch (names) {
    [] =>
      'No device gets notifications yet. Turn them on in Claktory on an '
          'iPhone, iPad or Mac.',
    [final one] => 'Sent to $one.',
    [...final rest, final last] => 'Sent to ${rest.join(', ')} and $last.',
  };
}

/// Whether the system lets Claktory notify on this device: a Mac's
/// (Notifications.swift), an iPhone's or iPad's (its push registration), a
/// fake in tests. None where notifications aren't shown yet (Android).
abstract interface class DeviceNotifications {
  /// "This Mac", "This iPhone", "This iPad".
  String get name;

  /// It gets notifications pushed while Claktory is closed on it, once it
  /// registered with this Claktory (an iPhone or iPad), rather than only
  /// while Claktory is open (a Mac).
  bool get pushes;

  /// "not_determined", "denied" or "allowed".
  Future<String> permission();

  /// Asks the person, the first time; answers [permission] after.
  Future<String> ask();

  /// The system's own settings for Claktory's notifications.
  Future<void> openSettings();

  /// Where setting notifications up failed, by the connection to each
  /// Claktory it failed for, until it works; null where nothing is set up
  /// with each Claktory (a Mac).
  ValueListenable<Map<Backend, PushStep>>? get setupFailures;

  /// Sets notifications up with [backend]'s Claktory again, now.
  void retrySetup(Backend backend);

  /// This device's, or null where there's nothing to show.
  static DeviceNotifications? of(BuildContext context) {
    if (debugOverride case final fake?) return fake;
    if (PushRegistrar.current case final registrar?) {
      final camera = AppScope.scope(context).camera;
      return PushNotifications(
        registrar: registrar,
        name: 'This $thisDeviceKind',
        openSettings: camera.openSettings,
      );
    }
    if (defaultTargetPlatform == TargetPlatform.macOS) {
      return const MacNotificationPermission();
    }
    return null;
  }

  /// Takes the place of this device's in tests.
  @visibleForTesting
  static DeviceNotifications? debugOverride;
}

/// A Mac's: shown while Claktory is open on it, as macOS allows.
class MacNotificationPermission implements DeviceNotifications {
  const MacNotificationPermission({
    this.channel = const MethodChannel('factory/notifications'),
  });

  final MethodChannel channel;

  @override
  String get name => 'This Mac';

  @override
  ValueListenable<Map<Backend, PushStep>>? get setupFailures => null;

  @override
  void retrySetup(Backend backend) {}

  @override
  bool get pushes => false;

  Future<String> _call(String method) async {
    try {
      return await channel.invokeMethod<String>(method) ?? 'not_determined';
    } on MissingPluginException {
      return 'not_determined';
    }
  }

  @override
  Future<String> permission() => _call('permission');

  @override
  Future<String> ask() => _call('ask');

  @override
  Future<void> openSettings() async {
    try {
      await channel.invokeMethod<void>('openSettings');
    } on MissingPluginException {
      // Nothing to open.
    }
  }
}

/// An iPhone's or iPad's: pushed through its registration with each
/// Claktory (mobile/push.dart).
class PushNotifications implements DeviceNotifications {
  PushNotifications({
    required this.registrar,
    required this.name,
    required this._openSettings,
  });

  final PushRegistrar registrar;
  final Future<void> Function() _openSettings;

  @override
  final String name;

  @override
  bool get pushes => true;

  @override
  Future<String> permission() async =>
      (await registrar.refresh())?.permission ?? 'not_determined';

  @override
  Future<String> ask() async =>
      (await registrar.ask())?.permission ?? 'not_determined';

  @override
  Future<void> openSettings() => _openSettings();

  @override
  ValueListenable<Map<Backend, PushStep>> get setupFailures =>
      registrar.failures;

  @override
  void retrySetup(Backend backend) => registrar.retry(backend);
}

/// This device's row: whether it gets notifications, what to do when it
/// doesn't (turn them on, or the system's Settings when they're off there),
/// and a test notification. Read again whenever the app comes back to the
/// front, so a change made in the system's Settings shows at once.
class ThisDeviceNotifications extends StatefulWidget {
  const ThisDeviceNotifications({super.key});

  @override
  State<ThisDeviceNotifications> createState() =>
      _ThisDeviceNotificationsState();
}

class _ThisDeviceNotificationsState extends State<ThisDeviceNotifications>
    with WidgetsBindingObserver, SettingRequests {
  DeviceNotifications? _device;

  /// The system's answer, once read.
  String? _permission;

  /// Where the test went, said once it was sent, since the last press.
  String? _sent;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_device != null) return;
    _device = DeviceNotifications.of(context);
    _read();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) _read();
  }

  Future<void> _read() async {
    final device = _device;
    if (device == null) return;
    final permission = await device.permission();
    if (mounted && permission != _permission) {
      setState(() => _permission = permission);
    }
  }

  Future<void> _ask() async {
    final device = _device;
    if (device == null) return;
    final permission = await device.ask();
    if (mounted) setState(() => _permission = permission);
  }

  Future<void> _test() async {
    setState(() => _sent = null);
    final backend = AppScope.of(context);
    await run(backend.sendTestNotification);
    if (!mounted || failure != null) return;
    setState(
      () => _sent = testSentTo(
        devices: backend.workspace.workspace.remote.devices,
        thisDevice: backend.deviceId,
        // "This Mac" → "this Mac", in the middle of a sentence.
        here: _permission == 'allowed'
            ? _device?.name.replaceFirst('This ', 'this ')
            : null,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final device = _device;
    final permission = _permission;
    final backend = AppScope.of(context);
    // Whether it's registered follows the Workspace (Device.notifications),
    // and whether setting it up failed, the registrar.
    return ListenableBuilder(
      listenable: Listenable.merge([backend.workspace, ?device?.setupFailures]),
      builder: (context, _) => _row(context, palette, type, device, permission),
    );
  }

  Widget _row(
    BuildContext context,
    Palette palette,
    TypeScale type,
    DeviceNotifications? device,
    String? permission,
  ) {
    final backend = AppScope.of(context);
    final registered =
        device != null &&
        device.pushes &&
        backend.workspace.workspace.remote.devices.any(
          (each) => each.id == backend.deviceId && each.notifications,
        );
    // Setting it up with this Claktory failed (#B3: v0.1.16's keychain).
    final failed = device?.setupFailures?.value.containsKey(backend) ?? false;
    final (String status, Widget? action) = switch (permission) {
      null => ('', null),
      'denied' => (
        defaultTargetPlatform == TargetPlatform.macOS
            ? 'Turned off for Claktory in System Settings.'
            : 'Turned off for Claktory in Settings.',
        Button(
          label: defaultTargetPlatform == TargetPlatform.macOS
              ? 'Open System Settings'
              : 'Open Settings',
          onPressed: () => unawaited(device!.openSettings()),
        ),
      ),
      'not_determined' => (
        'Not turned on yet.',
        Button(label: 'Turn on', onPressed: _ask),
      ),
      _ when failed && !registered => (
        "Couldn't set up notifications. Claktory tries again each time it "
            'connects.',
        Button(
          label: 'Try again',
          onPressed: () => device!.retrySetup(backend),
        ),
      ),
      _ => (
        device?.pushes != true
            ? 'Shows notifications while Claktory is open on it.'
            : registered
            ? 'Gets notifications, even when Claktory is closed.'
            : "Gets notifications once it's connected to this Claktory.",
        null,
      ),
    };
    final statusText = Text(
      status,
      style: type.body.copyWith(
        color: permission == 'denied' ? palette.textMuted : palette.text,
      ),
    );
    // Android shows no notifications yet (no pushes, nothing while it's
    // open either), so it says so rather than look set up (#352 P2).
    final none =
        device == null && defaultTargetPlatform == TargetPlatform.android;
    final test = Button(label: 'Send a test notification', onPressed: _test);
    return SettingRow(
      label: none ? 'This $thisDeviceKind' : device?.name ?? 'Test',
      top: true,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (device != null && status.isNotEmpty) ...[
            if (action == null)
              statusText
            else
              ActionLine(lead: statusText, action: action),
            const SizedBox(height: Space.m),
          ] else if (none) ...[
            Text(
              "This $thisDeviceKind doesn't get notifications yet. Open "
              "Claktory to see what's new.",
              style: type.body.copyWith(color: palette.text),
            ),
            const SizedBox(height: Space.m),
          ],
          Align(alignment: AlignmentDirectional.centerStart, child: test),
          const SizedBox(height: Space.xs),
          Text(
            _sent ??
                'It goes to every device that gets notifications, even in '
                    'quiet hours.',
            style: type.caption.copyWith(color: palette.textFaint),
          ),
          failureLine(palette),
        ],
      ),
    );
  }
}
