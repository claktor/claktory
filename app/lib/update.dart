import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart' show defaultTargetPlatform;
import 'package:flutter/widgets.dart';

import 'android/android_app_updates.dart';
import 'app_update.dart';
import 'backend.dart';
import 'changelog.dart';
import 'claktory_identity.dart' show claktoryName;
import 'compatibility.dart' show Mismatch, mismatch;
import 'controls.dart';
import 'devices.dart' show thisDeviceKind;
import 'identity.dart';
import 'mobile/lines.dart' show ActionLine;
import 'mobile/metrics.dart' show TypeScale;
import 'mobile/platform_policy.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'tell_rivet.dart';
import 'usage_wait.dart';

/// What this app knows about its own build, for offering and installing
/// updates.
class AppBuild {
  const AppBuild({
    required this.version,
    required this.bundlePath,
    required this.quit,
    this.updater = const AppUpdater(),
  });

  /// The running app: its stamped version, the bundle it runs from, and
  /// quitting by exiting the process.
  factory AppBuild.current() => AppBuild(
    version: const String.fromEnvironment(
      'FACTORY_VERSION',
      defaultValue: 'dev',
    ),
    bundlePath: bundlePathOf(Platform.resolvedExecutable),
    quit: () => exit(0),
  );

  /// Stamped by the build scripts, e.g. "2026.11.18-1652"; "dev" when not.
  final String version;

  /// The app bundle this runs from (…/Factory.app); empty when it isn't in
  /// one.
  final String bundlePath;

  /// Quits the app so its bundle can be replaced.
  final VoidCallback quit;

  /// Replaces this app with a Factory's on another machine.
  final AppUpdater updater;
}

/// The macOS bundle an executable is in: …/X.app/Contents/MacOS/X gives
/// …/X.app. Empty for an executable outside a bundle.
String bundlePathOf(String executable) {
  const inside = '.app/Contents/MacOS/';
  final index = executable.lastIndexOf(inside);
  return index < 0 ? '' : executable.substring(0, index + '.app'.length);
}

/// Whether [release] is newer than [version], by [compareVersions]. An
/// unstamped build ("dev") is never offered an update.
bool isNewer(String release, String version) =>
    release.isNotEmpty && compareVersions(release, version) > 0;

/// Orders versions by when they were made, as the backend does
/// (`update::compare_versions`): the old date stamps first, as text among
/// themselves; then numbered versions part by part (v0.1.10 after v0.1.9),
/// a build with "-…" after its numbers ("v0.1.3-dev.x") just before the
/// release it leads to, and builds by what follows the "-"; "dev" last.
int compareVersions(String a, String b) {
  final (rankA, rankB) = (_versionRank(a), _versionRank(b));
  if (rankA.$1 != rankB.$1) return rankA.$1.compareTo(rankB.$1);
  switch ((rankA.$2, rankB.$2)) {
    case (null, null):
      // Both date stamps, or both "dev".
      return a.compareTo(b);
    case ((final numbersA, final buildA)?, (final numbersB, final buildB)?):
      for (var i = 0; i < 3; i++) {
        final order = numbersA[i].compareTo(numbersB[i]);
        if (order != 0) return order;
      }
      if (buildA == null || buildB == null) {
        return buildA == buildB ? 0 : (buildA == null ? 1 : -1);
      }
      return buildA.compareTo(buildB);
    default:
      return 0;
  }
}

/// A version's kind (0 a date stamp, 1 numbered, 2 "dev") and, numbered,
/// its three numbers and the build after "-", if any.
(int, (List<BigInt>, String?)?) _versionRank(String version) {
  if (version == 'dev') return (2, null);
  final numbered = RegExp(
    r'^v(\d+)\.(\d+)\.(\d+)(?:-(.*))?$',
    dotAll: true,
  ).firstMatch(version);
  if (numbered == null) return (0, null);
  return (
    1,
    (
      [for (var i = 1; i <= 3; i++) BigInt.parse(numbered.group(i)!)],
      numbered.group(4),
    ),
  );
}

/// Whether to offer [Workspace.latestRelease]: the backend or this app is
/// older than it. Never in the Android app ([updatesHere]).
bool updateOffered(Workspace workspace, String appVersion) =>
    updatesHere &&
    (restartsBackend(workspace) ||
        isNewer(workspace.latestRelease, appVersion));

/// Whether this app replaces itself when it updates: a desktop's does (as
/// the banner's Update), a phone's or tablet's comes from its store, so
/// there Update updates only the Claktory.
bool get replacesItself => switch (defaultTargetPlatform) {
  TargetPlatform.macOS ||
  TargetPlatform.linux ||
  TargetPlatform.windows => true,
  _ => false,
};

/// The backend the update on offer brings: [Workspace.latestBackend], or
/// [Workspace.latestRelease] when it doesn't say.
String offeredBackend(Workspace workspace) => workspace.latestBackend.isEmpty
    ? workspace.latestRelease
    : workspace.latestBackend;

/// The paired devices, other than [thisDevice], whose app won't connect
/// after the update on offer until it's updated (#355 D1): the backend's
/// word (PairedDevice.update_leaves_behind), by its kept app versions.
List<PairedDevice> leftBehind(Workspace workspace, String thisDevice) => [
  for (final device in workspace.remote.devices)
    if (device.updateLeavesBehind && device.id != thisDevice) device,
];

/// What the update's offer says of [devices] ([leftBehind]); null when
/// there are none.
String? leftBehindText(List<PairedDevice> devices) =>
    switch ([for (final device in devices) device.name]) {
      [] => null,
      [final one] =>
        "$one won't connect after this update until its app is updated.",
      [...final rest, final last] =>
        "${rest.join(', ')} and $last won't connect after this update until "
            'their apps are updated.',
    };

/// Whether this device's own app can't follow the update on offer, where
/// the update doesn't replace it ([replacesApp] false: a phone's or
/// tablet's comes from its store): its own row says so
/// (PairedDevice.update_leaves_behind), or, with a backend too old to
/// say, its build is on another line than the backend on offer. Updating
/// would lock it out, so it isn't offered there (#355 D1).
bool cantFollowUpdate(
  Workspace workspace, {
  required String thisDevice,
  required String appVersion,
  required bool replacesApp,
}) {
  if (replacesApp || !restartsBackend(workspace)) return false;
  return workspace.remote.devices.any(
        (device) => device.id == thisDevice && device.updateLeavesBehind,
      ) ||
      mismatch(appVersion, offeredBackend(workspace)) == Mismatch.appOlder;
}

/// What a device that can't follow the update says in its place
/// ([cantFollowUpdate]): its app first, then the Claktory. The iPhone and
/// iPad app comes from the App Store or TestFlight; the Android app from
/// its download page, which Download beside it opens (Sale's row 60).
String updateAppFirstText(String claktory, String release) => updatesHere
    ? "Update this $thisDeviceKind's app first, from the App Store or "
          'TestFlight. Then $claktory can update to Claktory $release.'
    : "Update this $thisDeviceKind's app first: tap Download for its new "
          'version. Then $claktory can update to Claktory $release.';

/// Whether this app offers Claktory's updates and installs them: not the
/// Android app, whose new version is a new APK from its link, so it never
/// shows the update banner or touches the installer. The Android app is
/// Android with Android's own policy, a touch one (tests run as Android
/// with the desktop policy unless they turn Android's on).
bool get updatesHere =>
    defaultTargetPlatform != TargetPlatform.android ||
    !PhonePlatformPolicy.current.touch;

/// Whether installing the latest release replaces the backend, which
/// restarts it; a release that only changed the app leaves it running.
bool restartsBackend(Workspace workspace) => isNewer(
  workspace.latestBackend.isEmpty
      ? workspace.latestRelease
      : workspace.latestBackend,
  workspace.version,
);

/// What Update does, in the banner and in Settings: installs
/// [Workspace.latestRelease]. An app older than it is replaced too ([app]):
/// on a Claktory on another machine the app fetches the release's own,
/// with [onProgress] saying how far (null once it's done). Says whether
/// this app is being replaced, so it should quit now; throws what went
/// wrong.
///
/// There the Claktory updates first, since it has the release already,
/// and the app's download follows, over its restart and any drop of the
/// connection (#408): a slow link never holds the Claktory back. Only a
/// release this app can't work with (another line, [mismatch]) is
/// downloaded first, so the app never faces a Claktory it can't follow.
Future<bool> installLatest(
  AppScope scope,
  Workspace workspace, {
  void Function(UpdateProgress?)? onProgress,
  bool app = true,
}) async {
  if (_underWay case final running?) {
    // Already under way: this joins it, rather than a second download of
    // the same app into the same folder.
    void forward() => onProgress?.call(running.progress.value);
    forward();
    running.progress.addListener(forward);
    try {
      return await running.done;
    } finally {
      running.progress.removeListener(forward);
    }
  }
  final progress = ValueNotifier<UpdateProgress?>(null);
  void forward() => onProgress?.call(progress.value);
  progress.addListener(forward);
  final done = _install(
    scope,
    workspace,
    app: app,
    onProgress: (step) => progress.value = step,
  );
  final mine = (done: done, progress: progress);
  _underWay = mine;
  try {
    return await done;
  } finally {
    progress.removeListener(forward);
    if (identical(_underWay, mine)) _underWay = null;
  }
}

/// The update under way in this app, and how far it is. A banner or
/// Settings row made again meanwhile (the window rebuilt while the
/// Claktory restarts, say) joins it ([installLatest]).
({Future<bool> done, ValueNotifier<UpdateProgress?> progress})? _underWay;

/// Whether an update is under way in this app ([installLatest]).
bool get updateUnderWay => _underWay != null;

Future<bool> _install(
  AppScope scope,
  Workspace workspace, {
  required void Function(UpdateProgress?) onProgress,
  required bool app,
}) async {
  final build = scope.appBuild;
  final latest = workspace.latestRelease;
  // Only an app older than the release is replaced; the backend updates
  // itself whenever it's older. Without [app] (a phone, whose app isn't
  // Claktory's to replace), only the backend.
  final appOlder = app && isNewer(latest, build.version);
  final remote = scope.backend.target != null;
  // A Factory on another machine can't reach this app's bundle, so the
  // app fetches the release's own, checks it and swaps itself.
  final fetches = remote && appOlder;
  final appFirst =
      fetches && mismatch(build.version, offeredBackend(workspace)) != null;
  Future<StagedApp> fetch() async {
    final staged = await build.updater.stage(
      fetch: scope.backend.fetchApp,
      version: latest,
      bundlePath: build.bundlePath,
      connected: scope.backend.connected,
      onProgress: onProgress,
    );
    onProgress(null);
    return staged;
  }

  StagedApp? staged;
  try {
    if (appFirst) staged = await fetch();
    await scope.backend.installUpdate(
      !remote && appOlder ? build.bundlePath : '',
    );
    if (fetches && !appFirst) staged = await fetch();
    if (staged != null) {
      await build.updater.replace(staged, app: build.bundlePath);
    }
  } on Exception {
    if (staged != null) build.updater.discard(staged);
    rethrow;
  }
  return appOlder;
}

/// Why an update failed, in the person's words.
String updateFailureText(Exception failure) => switch (failure) {
  RequestFailure(:final message) => message,
  UpdateFailure(:final message) => message,
  _ => '$failure',
};

/// Says a new release is ready and installs it. Shown while the backend or
/// this app is older than the latest release; it can't be dismissed.
class UpdateBanner extends StatefulWidget {
  const UpdateBanner({super.key});

  @override
  State<UpdateBanner> createState() => _UpdateBannerState();
}

class _UpdateBannerState extends State<UpdateBanner> {
  var _updating = false;
  String? _failure;

  /// How far fetching the app from a Factory on another machine is.
  UpdateProgress? _progress;
  ValueNotifier<bool>? _connected;

  /// Re-enables Update if a backend that should restart doesn't.
  Timer? _restartTimeout;

  /// Update is under way: the Claktory updating, and on another machine
  /// this app's download after it.
  var _installing = false;

  @override
  void initState() {
    super.initState();
    // Made again while an update is under way: it shows that one.
    if (updateUnderWay) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && !_updating) {
          _update(AppScope.scope(context).backend.workspace.workspace);
        }
      });
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final connected = AppScope.of(context).connected;
    if (connected == _connected) return;
    _connected?.removeListener(_reconnected);
    _connected = connected..addListener(_reconnected);
  }

  @override
  void dispose() {
    _connected?.removeListener(_reconnected);
    _restartTimeout?.cancel();
    super.dispose();
  }

  /// The restarted backend is back. If it still needs updating, offer it
  /// again.
  void _reconnected() {
    // A download that carries on over the restart isn't done yet.
    if (_connected?.value == true && _updating && !_installing) {
      setState(() => _updating = false);
    }
  }

  Future<void> _update(Workspace workspace) async {
    final scope = AppScope.scope(context);
    final restarts = restartsBackend(workspace);
    setState(() {
      _updating = true;
      _failure = null;
      _progress = null;
    });
    final bool appReplaced;
    _installing = true;
    try {
      appReplaced = await installLatest(
        scope,
        workspace,
        onProgress: (progress) {
          if (mounted) setState(() => _progress = progress);
        },
      );
    } on Exception catch (failure) {
      if (!mounted) return;
      setState(() {
        _updating = false;
        _progress = null;
        _failure = updateFailureText(failure);
      });
      return;
    } finally {
      _installing = false;
    }
    if (appReplaced) {
      // This app's bundle is replaced once it's gone, then reopened: by the
      // backend on this Mac, or by the swap it just started.
      scope.appBuild.quit();
    } else if (!restarts) {
      if (mounted) setState(() => _updating = false);
    } else {
      // The backend restarts: the connection drops and comes back, and the
      // banner goes once the versions match.
      _restartTimeout?.cancel();
      _restartTimeout = Timer(const Duration(seconds: 15), () {
        if (mounted && _connected?.value == true) {
          setState(() => _updating = false);
        }
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final scope = AppScope.scope(context);
    final store = scope.backend.workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        final workspace = store.workspace;
        final latest = workspace.latestRelease;
        if (!updateOffered(workspace, scope.appBuild.version)) {
          return const SizedBox.shrink();
        }
        var working = 0;
        for (final agent in workspace.agents) {
          // One waiting out its usage limits isn't busy: it waits again
          // after the restart, as it would have.
          if (atWork(agent)) working++;
        }
        final thisDevice = scope.backend.deviceId;
        return _Strip(
          release: latest,
          behind: leftBehindText(leftBehind(workspace, thisDevice)),
          appFirst:
              cantFollowUpdate(
                workspace,
                thisDevice: thisDevice,
                appVersion: scope.appBuild.version,
                replacesApp: replacesItself,
              )
              ? updateAppFirstText(claktoryName(scope.backend), latest)
              : null,
          working: working,
          restarts: restartsBackend(workspace),
          undone: workspace.updateError,
          updating: _updating,
          failure: _failure,
          progress: _progress,
          machine: workspace.remote.machine.isNotEmpty
              ? workspace.remote.machine
              : scope.backend.target?.label ?? '',
          onUpdate: _updating ? null : () => _update(workspace),
          news: newVersions(
            workspace,
            scope.appBuild.version,
          ).any((version) => version.items.isNotEmpty),
        );
      },
    );
  }
}

class _Strip extends StatelessWidget {
  const _Strip({
    required this.release,
    required this.working,
    required this.restarts,
    required this.undone,
    required this.updating,
    required this.failure,
    required this.onUpdate,
    this.progress,
    this.machine = '',
    this.news = false,
    this.behind,
    this.appFirst,
  });

  /// The paired devices the update would leave behind, said before Update
  /// ([leftBehindText]); null when none.
  final String? behind;

  /// This device can't follow the update (its app is from its store): what
  /// to do first, said in place of the offer, with no Update.
  final String? appFirst;

  /// How far fetching the app from the other machine is, while it is.
  final UpdateProgress? progress;

  /// That machine's name.
  final String machine;

  /// The changelog says what the update brings, so What's new can show it.
  final bool news;

  final String release;
  final int working;

  /// Updating restarts the backend (not just the app).
  final bool restarts;

  /// Why the last update was undone; empty when it wasn't.
  final String undone;
  final bool updating;
  final String? failure;
  final VoidCallback? onUpdate;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final stops = !restarts
        ? 'Only the app restarts; agents keep working.'
        : switch (working) {
            0 => null,
            1 => 'The working agent picks up where it left off.',
            _ => 'Working agents pick up where they left off.',
          };
    // The top of the window, so it drags it like a title bar.
    return WindowDrag(
      child: Container(
        constraints: const BoxConstraints(minHeight: titleBarHeight),
        padding: const EdgeInsets.fromLTRB(Space.xl, Space.s, Space.m, Space.s),
        decoration: BoxDecoration(
          color: palette.sidebar,
          border: Border(bottom: BorderSide(color: palette.hairline)),
        ),
        child: Row(
          children: [
            SizedBox(
              width: Space.l,
              child: Align(
                alignment: Alignment.centerLeft,
                child: updating ? const LiveDot() : Dot(color: palette.signal),
              ),
            ),
            Expanded(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text.rich(
                    TextSpan(
                      children: [
                        TextSpan(
                          text: appFirst ?? 'Claktory $release is ready.',
                          style: TextStyles.label.copyWith(color: palette.text),
                        ),
                        if (stops != null && appFirst == null)
                          TextSpan(
                            text: ' $stops',
                            style: TextStyles.body.copyWith(
                              color: palette.textMuted,
                            ),
                          ),
                      ],
                    ),
                  ),
                  if (behind case final behind? when appFirst == null)
                    Text(
                      behind,
                      style: TextStyles.caption.copyWith(
                        color: palette.textMuted,
                      ),
                    ),
                  if (undone.isNotEmpty)
                    Text(
                      undone,
                      style: TextStyles.caption.copyWith(color: palette.danger),
                    ),
                  if (progress case final progress?)
                    _Progress(progress: progress, machine: machine),
                  if (failure != null)
                    Text(
                      "Couldn't update: $failure",
                      style: TextStyles.caption.copyWith(color: palette.danger),
                    ),
                ],
              ),
            ),
            const SizedBox(width: Space.m),
            if (!updating && (failure != null || undone.isNotEmpty)) ...[
              TellRivet(
                error: failure ?? undone,
                about: failure != null
                    ? "Updating to Claktory $release didn't work."
                    : 'Updating to Claktory $release was undone.',
              ),
              const SizedBox(width: Space.xs),
            ],
            if (news) ...[
              Button(
                label: "What's new",
                onPressed: () => AppScope.openChangelog(context),
              ),
              const SizedBox(width: Space.xs),
            ],
            if (appFirst == null)
              Button(
                label: updating
                    ? 'Updating…'
                    : undone.isNotEmpty || failure != null
                    ? 'Try again'
                    : 'Update',
                onPressed: onUpdate,
                primary: true,
              ),
          ],
        ),
      ),
    );
  }
}

/// Fetching the app from a Factory on another machine: what it's doing, and
/// while it downloads, a meter and how much has come.
class _Progress extends StatelessWidget {
  const _Progress({required this.progress, required this.machine});

  final UpdateProgress progress;
  final String machine;

  static String _megabytes(int bytes) => (bytes / 1e6).toStringAsFixed(1);

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final where = machine.isEmpty ? 'the other machine' : machine;
    final text = switch (progress.stage) {
      UpdateStage.preparing => 'Getting the app ready on $where…',
      UpdateStage.downloading =>
        'Downloading the app from $where: '
            '${_megabytes(progress.received)} of '
            '${_megabytes(progress.size)} MB',
      UpdateStage.waiting when progress.size == 0 =>
        'Waiting for the connection to $where…',
      UpdateStage.waiting =>
        'Waiting for the connection to $where to carry on: '
            '${_megabytes(progress.received)} of '
            '${_megabytes(progress.size)} MB so far',
      UpdateStage.checking => 'Checking the new app…',
    };
    return Padding(
      padding: const EdgeInsets.only(top: Space.xs),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (progress.size > 0 &&
              progress.stage != UpdateStage.preparing &&
              progress.stage != UpdateStage.checking) ...[
            Container(
              width: 48,
              height: 4,
              alignment: Alignment.centerLeft,
              decoration: BoxDecoration(
                color: palette.selected,
                borderRadius: BorderRadius.circular(2),
              ),
              child: FractionallySizedBox(
                widthFactor: progress.fraction,
                heightFactor: 1,
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    color: palette.signal,
                    borderRadius: BorderRadius.circular(2),
                  ),
                ),
              ),
            ),
            const SizedBox(width: Space.s),
          ],
          Flexible(
            child: Text(
              text,
              style: TextStyles.caption.copyWith(
                color: palette.textMuted,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// The Android app's check for a new version of itself (#355 D2, Doppio's
/// android_app_updates.dart), for what's below it to show
/// ([AndroidUpdateNotice]): read as the app starts and each time it comes
/// back to the front, never polled or kept. Off outside a release build on
/// Android, where it finds nothing.
class AndroidAppUpdateHost extends StatefulWidget {
  const AndroidAppUpdateHost({super.key, required this.child});

  final Widget child;

  /// The check, where there is one.
  static AndroidAppUpdates? of(BuildContext context) => context
      .dependOnInheritedWidgetOfExactType<_AndroidAppUpdatesScope>()
      ?.notifier;

  /// Makes the check in tests in place of the real one.
  @visibleForTesting
  static AndroidAppUpdates Function()? debugMake;

  @override
  State<AndroidAppUpdateHost> createState() => _AndroidAppUpdateHostState();
}

class _AndroidAppUpdateHostState extends State<AndroidAppUpdateHost>
    with WidgetsBindingObserver {
  late final _updates =
      (AndroidAppUpdateHost.debugMake ?? AndroidAppUpdates.new)();

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    unawaited(_updates.refresh());
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) unawaited(_updates.refresh());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _updates.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) =>
      _AndroidAppUpdatesScope(notifier: _updates, child: widget.child);
}

class _AndroidAppUpdatesScope extends InheritedNotifier<AndroidAppUpdates> {
  const _AndroidAppUpdatesScope({
    required super.notifier,
    required super.child,
  });
}

/// What [AndroidUpdateNotice] says: with this app's own version when it's
/// stamped, since in Settings it sits under the Claktory's (Sale's row,
/// Trish's words).
String androidUpdateText(String appVersion) =>
    RegExp(r'^v\d').hasMatch(appVersion)
    ? 'This app is $appVersion. A new version is available.'
    : 'A new version of this app is available.';

/// On Android, while a newer build of this app is out that works with the
/// Claktory it shows: says so, with Download, which opens its download page
/// in the browser (nothing installs by itself). Nothing otherwise.
class AndroidUpdateNotice extends StatefulWidget {
  const AndroidUpdateNotice({super.key, this.padding = EdgeInsets.zero});

  /// Around it, only when it shows.
  final EdgeInsetsGeometry padding;

  /// Opens a page in tests in place of the browser.
  @visibleForTesting
  static Future<bool> Function(Uri)? debugOpen;

  @override
  State<AndroidUpdateNotice> createState() => _AndroidUpdateNoticeState();
}

class _AndroidUpdateNoticeState extends State<AndroidUpdateNotice> {
  /// The browser didn't open the page, since the last tap.
  var _failed = false;

  /// Asking the browser to open it.
  var _opening = false;

  Future<void> _open(AndroidAppUpdate offer) async {
    setState(() {
      _failed = false;
      _opening = true;
    });
    final opened = await openAndroidAppUpdatePage(
      page: offer.page,
      open: AndroidUpdateNotice.debugOpen,
    );
    if (!mounted) return;
    setState(() {
      _opening = false;
      _failed = !opened;
    });
  }

  @override
  Widget build(BuildContext context) {
    final updates = AndroidAppUpdateHost.of(context);
    if (updates == null) return const SizedBox.shrink();
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        final offer = androidAppUpdate(
          latest: updates.latest,
          installedVersionCode: updates.installedVersionCode,
          backendVersion: store.workspace.version,
        );
        if (offer == null) return const SizedBox.shrink();
        final palette = Palette.of(context);
        final type = TypeScale.of(context);
        return Padding(
          padding: widget.padding,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              ActionLine(
                lead: Text(
                  androidUpdateText(AppScope.scope(context).appBuild.version),
                  style: type.body.copyWith(color: palette.text),
                ),
                action: Button(
                  label: _opening ? 'Opening…' : 'Download',
                  onPressed: _opening ? null : () => unawaited(_open(offer)),
                ),
              ),
              if (_failed)
                Text(
                  "Couldn't open the download page. Try again.",
                  style: type.caption.copyWith(color: palette.danger),
                ),
            ],
          ),
        );
      },
    );
  }
}
