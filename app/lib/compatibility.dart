import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'claktory_identity.dart';
import 'identity.dart';
import 'name_step.dart' show StepCard;
import 'scope.dart';
import 'update.dart';

/// Which side is too old when this app and its Claktory's backend can't
/// work together.
enum Mismatch {
  /// The backend is on an older line than this app: update Claktory.
  backendOlder,

  /// This app is on an older line than the backend: update the app.
  appOlder,
}

/// A numbered version's line, its first two numbers: v0.1.11 and
/// v0.1.12-dev.x are both on 0.1. A release stamped `--incompatible` starts
/// a new line (v0.1.7 → v0.2.0), and only versions on one line work
/// together. Null for what isn't numbered ("dev", the date stamps from
/// before v0.1.0).
(BigInt, BigInt)? versionLine(String version) {
  final numbered = RegExp(r'^v(\d+)\.(\d+)\.\d+(?:-.*)?$').firstMatch(version);
  if (numbered == null) return null;
  return (BigInt.parse(numbered.group(1)!), BigInt.parse(numbered.group(2)!));
}

/// Whether an app at [appVersion] and a backend at [backendVersion]
/// (Workspace.version) can't work together, and which is too old. Null
/// when they can, or when either isn't numbered: a dev build, or a backend
/// from before v0.1.0, which no release has called incompatible.
Mismatch? mismatch(String appVersion, String backendVersion) {
  final app = versionLine(appVersion);
  final backend = versionLine(backendVersion);
  if (app == null || backend == null || app == backend) return null;
  final older =
      backend.$1 < app.$1 || (backend.$1 == app.$1 && backend.$2 < app.$2);
  return older ? Mismatch.backendOlder : Mismatch.appOlder;
}

/// The mismatch between this app and the Claktory it shows, once its
/// workspace has come.
Mismatch? mismatchIn(BuildContext context) {
  final scope = AppScope.scope(context);
  final store = scope.backend.workspace;
  if (!store.received) return null;
  return mismatch(scope.appBuild.version, store.workspace.version);
}

/// Over the whole window while this app and its Claktory can't work
/// together: which to update, and how. The Update banner stays clickable
/// above it where it can do it.
class VersionStep extends StatelessWidget {
  const VersionStep({super.key, required this.which});

  final Mismatch which;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    final workspace = scope.backend.workspace.workspace;
    final appVersion = scope.appBuild.version;
    final machine = claktoryName(scope.backend);
    final phone = defaultTargetPlatform == TargetPlatform.iOS;
    // The banner shows where the latest release fixes it: it updates the
    // backend, and on a Mac this app too.
    final latest = versionLine(workspace.latestRelease);
    // No banner on a phone: there the backend's own device updates it; nor
    // in the Android app, which never updates Claktory ([updatesHere]).
    final banner = !isPhoneLayout(context) && updatesHere;
    final fixes = switch (which) {
      Mismatch.backendOlder =>
        banner &&
            latest != null &&
            mismatch(appVersion, workspace.latestRelease) == null &&
            restartsBackend(workspace),
      Mismatch.appOlder =>
        banner &&
            !phone &&
            latest != null &&
            latest == versionLine(workspace.version) &&
            isNewer(workspace.latestRelease, appVersion),
    };
    final press = phone ? 'Tap' : 'Click';
    final (title, what, how) = switch (which) {
      Mismatch.backendOlder => (
        'Update Claktory to use this app',
        'This app is $appVersion, but Claktory on $machine is still '
            '${workspace.version}.',
        fixes
            ? '$press Update above to update it.'
            : 'Update Claktory on $machine, then come back.',
      ),
      Mismatch.appOlder => (
        'This app is too old for this Claktory',
        'This app is $appVersion, but Claktory on $machine is already '
            '${workspace.version}.',
        fixes
            ? '$press Update above to update the app.'
            : phone
            ? 'Update Claktory from the App Store or TestFlight, then open '
                  'it again.'
            : 'Update this app, then open it again.',
      ),
    };
    return StepCard(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(title, style: TextStyles.emphasis.copyWith(color: palette.text)),
          const SizedBox(height: Space.s),
          Text(what, style: TextStyles.body.copyWith(color: palette.textMuted)),
          const SizedBox(height: Space.s),
          Text(how, style: TextStyles.body.copyWith(color: palette.text)),
        ],
      ),
    );
  }
}
