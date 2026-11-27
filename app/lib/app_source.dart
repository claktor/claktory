// Where this app was installed from: the version rows
// of a phone, an iPad or Android offer the store that updates it, and none
// on a dev build. The native side answers `appSource` on factory/device:
// iOS from StoreKit 2's AppTransaction (AppDelegate.swift), Android from
// the installer package (AppSource.kt).
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'scope.dart';

/// Where the app came from, as the native side says.
enum AppSource {
  appStore('App Store'),
  testFlight('TestFlight'),
  googlePlay('Google Play'),

  /// claktory.com's Android build: its update is the version row's own
  /// Download, so it has no button of its own.
  website('');

  const AppSource(this.label);

  /// The button's words; empty where there's no button.
  final String label;

  static AppSource? parse(String? name) => switch (name) {
    'app_store' => appStore,
    'testflight' => testFlight,
    'google_play' => googlePlay,
    'website' => website,
    _ => null,
  };
}

/// Claktory's page in the App Store and its TestFlight invitation
/// .
const appStoreUrl = 'https://apps.apple.com/app/id6837053917';
const testFlightUrl = 'https://testflight.apple.com/join/kSYkdayX';
const googlePlayUrl =
    'https://play.google.com/store/apps/details?id=com.claktory.app';

const _channel = MethodChannel('factory/device');

/// Asked once a run: the store doesn't change while the app runs, and
/// iOS's first answer can take a moment.
Future<AppSource?>? _asked;

Future<AppSource?> appSource() {
  if (debugAppSource case final fake?) return Future.value(fake);
  return _asked ??= () async {
    if (kIsWeb ||
        (defaultTargetPlatform != TargetPlatform.iOS &&
            defaultTargetPlatform != TargetPlatform.android)) {
      return null;
    }
    try {
      return AppSource.parse(await _channel.invokeMethod<String>('appSource'));
    } on PlatformException {
      return null;
    } on MissingPluginException {
      return null;
    }
  }();
}

/// Takes the place of the native answer in tests.
@visibleForTesting
AppSource? debugAppSource;

/// Opens the store [source] names at Claktory: Google Play through the
/// Play app where Android has it (openAppSource), else its page.
Future<void> openAppSource(BuildContext context, AppSource source) async {
  final login = AppScope.scope(context).login;
  switch (source) {
    case AppSource.appStore:
      await login.open(appStoreUrl);
    case AppSource.testFlight:
      await login.open(testFlightUrl);
    case AppSource.googlePlay:
      bool opened;
      try {
        opened =
            await _channel.invokeMethod<bool>('openAppSource', 'google_play') ??
            false;
      } on PlatformException {
        opened = false;
      } on MissingPluginException {
        opened = false;
      }
      if (!opened) await login.open(googlePlayUrl);
    case AppSource.website:
      break;
  }
}

/// The button to the store this app came from ("App Store", "TestFlight",
/// "Google Play"); nothing on a dev build, from claktory.com, or until
/// the native side answers.
class AppSourceButton extends StatefulWidget {
  const AppSourceButton({super.key, this.primary = false});

  final bool primary;

  @override
  State<AppSourceButton> createState() => _AppSourceButtonState();
}

class _AppSourceButtonState extends State<AppSourceButton> {
  AppSource? _source = debugAppSource;

  @override
  void initState() {
    super.initState();
    appSource().then((source) {
      if (mounted && source != _source) setState(() => _source = source);
    });
  }

  @override
  Widget build(BuildContext context) {
    final source = _source;
    if (source == null || source.label.isEmpty) return const SizedBox.shrink();
    return Button(
      label: source.label,
      primary: widget.primary,
      onPressed: () => openAppSource(context, source),
    );
  }
}
