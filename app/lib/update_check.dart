// Settings → General's Version row: the version, Check for updates, and
// what the check found (Workspace.update_check), with Update when a newer
// release is ready.

import 'dart:async';

import 'package:flutter/foundation.dart' show defaultTargetPlatform;
import 'package:flutter/widgets.dart';

import 'app_update.dart' show UpdateProgress, UpdateStage;
import 'changelog.dart' show installedVersion;
import 'claktory_identity.dart' show claktoryName;
import 'controls.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'settings_view.dart' show SettingRequests, SettingRow;
import 'update.dart';

/// Whether this app replaces itself when it updates: a desktop's does (as
/// the banner's Update), a phone's or tablet's comes from its store, so
/// there Update updates only the Claktory.
bool get replacesItself => switch (defaultTargetPlatform) {
  TargetPlatform.macOS ||
  TargetPlatform.linux ||
  TargetPlatform.windows => true,
  _ => false,
};

/// What a check found, in the person's words, and whether Update goes with
/// it; null when there's nothing to say (no check yet).
({String text, bool ready, bool failed})? updateCheckAnswer({
  required UpdateCheck check,
  required Workspace workspace,
  required String appVersion,
  required bool replacesApp,
  required String claktory,
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
  return switch (check.state) {
    UpdateCheckState.UPDATE_CHECK_STATE_CHECKING => (
      text: 'Checking for updates…',
      ready: false,
      failed: false,
    ),
    UpdateCheckState.UPDATE_CHECK_STATE_DOWNLOADING => (
      text: 'Downloading Claktory ${check.version}…',
      ready: false,
      failed: false,
    ),
    UpdateCheckState.UPDATE_CHECK_STATE_UP_TO_DATE ||
    UpdateCheckState.UPDATE_CHECK_STATE_READY =>
      offered
          ? (text: ready(), ready: true, failed: false)
          : (text: 'Claktory is up to date.', ready: false, failed: false),
    UpdateCheckState.UPDATE_CHECK_STATE_FAILED => (
      text: check.error.isEmpty ? "Couldn't check for updates." : check.error,
      ready: false,
      failed: true,
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

  @override
  State<UpdateCheckRow> createState() => _UpdateCheckRowState();
}

class _UpdateCheckRowState extends State<UpdateCheckRow> with SettingRequests {
  var _updating = false;
  String? _updateFailure;
  UpdateProgress? _progress;

  /// Lets Update be pressed again if a backend that should restart doesn't.
  Timer? _restartTimeout;

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
                      version,
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
                  ],
                ),
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
            ],
          ),
        ),
        failureLine(palette),
      ],
    );
  }
}
