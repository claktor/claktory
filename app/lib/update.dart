import 'dart:async';
import 'dart:io';

import 'package:flutter/widgets.dart';

import 'app_update.dart';
import 'backend.dart';
import 'changelog.dart';
import 'controls.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'tell_rivet.dart';

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
/// older than it.
bool updateOffered(Workspace workspace, String appVersion) =>
    restartsBackend(workspace) || isNewer(workspace.latestRelease, appVersion);

/// Whether installing the latest release replaces the backend, which
/// restarts it; a release that only changed the app leaves it running.
bool restartsBackend(Workspace workspace) => isNewer(
  workspace.latestBackend.isEmpty
      ? workspace.latestRelease
      : workspace.latestBackend,
  workspace.version,
);

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
    if (_connected?.value == true && _updating) {
      setState(() => _updating = false);
    }
  }

  Future<void> _update(Workspace workspace) async {
    final scope = AppScope.scope(context);
    final build = scope.appBuild;
    final latest = workspace.latestRelease;
    // Only an app older than the release is replaced; the backend updates
    // itself whenever it's older.
    final appOlder = isNewer(latest, build.version);
    final remote = scope.backend.target != null;
    final restarts = restartsBackend(workspace);
    setState(() {
      _updating = true;
      _failure = null;
      _progress = null;
    });
    StagedApp? staged;
    try {
      if (remote && appOlder) {
        // A Factory on another machine can't reach this app's bundle, so
        // the app fetches the release's own, checks it and swaps itself.
        staged = await build.updater.stage(
          fetch: scope.backend.fetchApp,
          version: latest,
          bundlePath: build.bundlePath,
          onProgress: (progress) {
            if (mounted) setState(() => _progress = progress);
          },
        );
        if (mounted) setState(() => _progress = null);
      }
      await scope.backend.installUpdate(
        !remote && appOlder ? build.bundlePath : '',
      );
      if (staged != null) {
        await build.updater.replace(staged, app: build.bundlePath);
      }
    } on Exception catch (failure) {
      if (staged != null) build.updater.discard(staged);
      if (!mounted) return;
      setState(() {
        _updating = false;
        _progress = null;
        _failure = switch (failure) {
          RequestFailure(:final message) => message,
          UpdateFailure(:final message) => message,
          _ => '$failure',
        };
      });
      return;
    }
    if (appOlder) {
      // This app's bundle is replaced once it's gone, then reopened: by the
      // backend on this Mac, or by the swap it just started.
      build.quit();
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
          if (agent.status == AgentStatus.AGENT_STATUS_WORKING) working++;
        }
        return _Strip(
          release: latest,
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
  });

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
                          text: 'Claktory $release is ready.',
                          style: TextStyles.label.copyWith(color: palette.text),
                        ),
                        if (stops != null)
                          TextSpan(
                            text: ' $stops',
                            style: TextStyles.body.copyWith(
                              color: palette.textMuted,
                            ),
                          ),
                      ],
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
      UpdateStage.checking => 'Checking the new app…',
    };
    return Padding(
      padding: const EdgeInsets.only(top: Space.xs),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (progress.stage == UpdateStage.downloading) ...[
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
