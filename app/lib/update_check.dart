// Settings → General's Version row: the version, Check for updates, and
// what the check found (Workspace.update_check), with Update when a newer
// release is ready.

import 'dart:async';

import 'package:flutter/widgets.dart';

import 'android/android_app_updates.dart' show openAndroidAppUpdatePage;
import 'app_source.dart' show AppSourceButton;
import 'app_update.dart' show UpdateProgress, UpdateStage;
import 'changelog.dart' show installedVersion;
import 'claktory_identity.dart' show claktoryName, claktoryNameInSettings;
import 'controls.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'settings_view.dart' show SettingRequests, SettingRow, versionLabel;
import 'update.dart';

/// What a check found, in the person's words, and whether Update goes with
/// it; null when there's nothing to say (no check yet). Ready, [behind]
/// names the paired devices the update would leave behind; and on a device
/// that can't follow it ([cantFollowUpdate]: [thisDevice], a phone or
/// tablet), it says to update the app first ([appFirst]), and there's no
/// Update: in the Android app, with Download for the app's new version.
({String text, bool ready, bool failed, String? behind, bool appFirst})?
updateCheckAnswer({
  required UpdateCheck check,
  required Workspace workspace,
  required String appVersion,
  required bool replacesApp,
  required String claktory,
  String thisDevice = '',
}) {
  // Newer than what runs here: the backend's, and with [replacesApp] this
  // app's too (after an app-only release the backend still says ready, and
  // for an app already on it, it's up to date).
  final offered = replacesApp
      ? updateOffered(workspace, appVersion)
      : restartsBackend(workspace);
  final latest = workspace.latestRelease.isNotEmpty
      ? workspace.latestRelease
      : check.version;
  String ready() => replacesApp
      ? 'Claktory $latest is ready.'
      : '$claktory can update to Claktory $latest.';
  final cantFollow = cantFollowUpdate(
    workspace,
    thisDevice: thisDevice,
    appVersion: appVersion,
    replacesApp: replacesApp,
  );
  final behind = leftBehindText(leftBehind(workspace, thisDevice));
  return switch (check.state) {
    UpdateCheckState.UPDATE_CHECK_STATE_CHECKING => (
      text: 'Checking for updates…',
      ready: false,
      failed: false,
      behind: null,
      appFirst: false,
    ),
    UpdateCheckState.UPDATE_CHECK_STATE_DOWNLOADING => (
      text: 'Downloading Claktory ${check.version}…',
      ready: false,
      failed: false,
      behind: null,
      appFirst: false,
    ),
    UpdateCheckState.UPDATE_CHECK_STATE_UP_TO_DATE ||
    UpdateCheckState.UPDATE_CHECK_STATE_READY =>
      !offered
          ? (
              text: 'Claktory is up to date.',
              ready: false,
              failed: false,
              behind: null,
              appFirst: false,
            )
          : cantFollow
          ? (
              text: updateAppFirstText(claktory, latest),
              ready: false,
              failed: false,
              behind: null,
              appFirst: true,
            )
          : (
              text: ready(),
              ready: true,
              failed: false,
              behind: behind,
              appFirst: false,
            ),
    UpdateCheckState.UPDATE_CHECK_STATE_FAILED => (
      text: check.error.isEmpty ? "Couldn't check for updates." : check.error,
      ready: false,
      failed: true,
      behind: null,
      appFirst: false,
    ),
    _ => null,
  };
}

/// The Version row: the version running here, Check for updates, and the
/// answer in place under them, with Update when a release is ready. The
/// same check on every device: the backend's.
class UpdateCheckRow extends StatefulWidget {
  const UpdateCheckRow({super.key, required this.workspace});

  final Workspace workspace;

  /// Opens a page in tests in place of the browser.
  @visibleForTesting
  static Future<bool> Function(Uri)? debugOpen;

  @override
  State<UpdateCheckRow> createState() => _UpdateCheckRowState();
}

class _UpdateCheckRowState extends State<UpdateCheckRow> with SettingRequests {
  var _updating = false;
  String? _updateFailure;
  UpdateProgress? _progress;

  /// Lets Update be pressed again if a backend that should restart doesn't.
  Timer? _restartTimeout;

  /// Android, its app first: asking the browser for the download page, and
  /// whether it didn't open, since the last tap.
  var _opening = false;
  var _openFailed = false;

  Future<void> _download() async {
    setState(() {
      _opening = true;
      _openFailed = false;
    });
    final opened = await openAndroidAppUpdatePage(
      open: UpdateCheckRow.debugOpen,
    );
    if (!mounted) return;
    setState(() {
      _opening = false;
      _openFailed = !opened;
    });
  }

  @override
  void dispose() {
    _restartTimeout?.cancel();
    super.dispose();
  }

  Future<void> _update() async {
    final scope = AppScope.scope(context);
    final workspace = widget.workspace;
    setState(() {
      _updating = true;
      _updateFailure = null;
      _progress = null;
    });
    final bool appReplaced;
    try {
      appReplaced = await installLatest(
        scope,
        workspace,
        app: replacesItself,
        onProgress: (progress) {
          if (mounted) setState(() => _progress = progress);
        },
      );
    } on Exception catch (failure) {
      if (!mounted) return;
      setState(() {
        _updating = false;
        _progress = null;
        _updateFailure = updateFailureText(failure);
      });
      return;
    }
    if (appReplaced) {
      // As the banner's: the app's bundle is replaced once it's gone.
      scope.appBuild.quit();
      return;
    }
    // The backend restarts (or, with nothing of its own to change, is
    // done): the answer follows once it's back.
    _restartTimeout?.cancel();
    _restartTimeout = Timer(const Duration(seconds: 15), () {
      if (mounted) setState(() => _updating = false);
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final scope = AppScope.scope(context);
    final workspace = widget.workspace;
    final check = workspace.updateCheck;
    final replacesApp = replacesItself;
    // This app's version, or the Claktory's where it doesn't replace itself
    // (a phone: what Check for updates updates).
    final version = replacesApp
        ? installedVersion(workspace, scope.appBuild.version)
        : workspace.version;
    final answer = updateCheckAnswer(
      check: check,
      workspace: workspace,
      appVersion: scope.appBuild.version,
      replacesApp: replacesApp,
      claktory: claktoryName(scope.backend),
      thisDevice: scope.backend.deviceId,
    );
    final busy =
        check.state == UpdateCheckState.UPDATE_CHECK_STATE_CHECKING ||
        check.state == UpdateCheckState.UPDATE_CHECK_STATE_DOWNLOADING;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SettingRow(
          label: 'Version',
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Wrap(
                spacing: Space.m,
                runSpacing: Space.s,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  if (version.isNotEmpty)
                    Text(
                      // On a phone or an iPad, the computer's: named, since
                      // This iPhone has the app's own.
                      replacesApp
                          ? versionLabel(version)
                          : '${claktoryNameInSettings(scope.backend)} · Claktory '
                                '${versionLabel(version)}',
                      style: type.body.copyWith(
                        color: palette.text,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
                  Button(
                    label: 'Check for updates',
                    onPressed: busy || _updating
                        ? null
                        : () => run(AppScope.of(context).checkForUpdates),
                  ),
                ],
              ),
              if (answer != null) ...[
                const SizedBox(height: Space.s),
                if (answer.behind case final behind?) ...[
                  Text(
                    answer.text,
                    style: type.caption.copyWith(color: palette.text),
                  ),
                  const SizedBox(height: Space.xs),
                  Text(
                    behind,
                    style: type.caption.copyWith(color: palette.textMuted),
                  ),
                  const SizedBox(height: Space.s),
                  Button(
                    label: _updating ? 'Updating…' : 'Update',
                    primary: true,
                    onPressed: _updating ? null : _update,
                  ),
                ] else
                  Wrap(
                    spacing: Space.m,
                    runSpacing: Space.s,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: [
                      Text(
                        answer.text,
                        style: type.caption.copyWith(
                          color: answer.failed
                              ? palette.danger
                              : answer.ready
                              ? palette.text
                              : palette.textMuted,
                        ),
                      ),
                      if (answer.ready)
                        Button(
                          label: _updating ? 'Updating…' : 'Update',
                          primary: true,
                          onPressed: _updating ? null : _update,
                        ),
                      // An iPhone's or iPad's from its store, the one it
                      // came from; none on a dev build.
                      if (answer.appFirst && updatesHere && !replacesApp)
                        const AppSourceButton(primary: true),
                      // Android's app comes from its download page, not a
                      // store.
                      if (answer.appFirst && !updatesHere)
                        Button(
                          label: _opening ? 'Opening…' : 'Download',
                          primary: true,
                          onPressed: _opening ? null : _download,
                        ),
                    ],
                  ),
                // What the browser's download may ask on Android.
                if (answer.appFirst && !updatesHere) ...[
                  const SizedBox(height: Space.xs),
                  Text(
                    'Android may ask you to let your browser install apps.',
                    style: type.caption.copyWith(color: palette.textMuted),
                  ),
                ],
                if (_openFailed && answer.appFirst) ...[
                  const SizedBox(height: Space.xs),
                  Text(
                    "Couldn't open the download page. Try again.",
                    style: type.caption.copyWith(color: palette.danger),
                  ),
                ],
              ],
              if (_progress case final progress? when _updating) ...[
                const SizedBox(height: Space.xs),
                Text(
                  switch (progress.stage) {
                    UpdateStage.preparing => 'Getting the app ready…',
                    UpdateStage.downloading =>
                      'Downloading the app: '
                          '${(progress.received / 1e6).toStringAsFixed(1)} of '
                          '${(progress.size / 1e6).toStringAsFixed(1)} MB',
                    UpdateStage.waiting when progress.size == 0 =>
                      'Waiting for the connection…',
                    UpdateStage.waiting =>
                      'Waiting for the connection to carry on: '
                          '${(progress.received / 1e6).toStringAsFixed(1)} of '
                          '${(progress.size / 1e6).toStringAsFixed(1)} MB so far',
                    UpdateStage.checking => 'Checking the new app…',
                  },
                  style: type.caption.copyWith(
                    color: palette.textMuted,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
              ],
              if (_updateFailure != null) ...[
                const SizedBox(height: Space.xs),
                Text(
                  "Couldn't update: $_updateFailure",
                  style: type.caption.copyWith(color: palette.danger),
                ),
              ],
              // On Android, a new version of this app.
              const AndroidUpdateNotice(padding: EdgeInsets.only(top: Space.s)),
            ],
          ),
        ),
        failureLine(palette),
      ],
    );
  }
}
