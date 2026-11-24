// Removing Claktory from this Mac, from Settings → This Mac: the installed
// backend's `factoryd uninstall` stops the agents and takes Claktory away,
// keeping the person's data or deleting it too; then the app puts itself
// in the Trash and quits (app/macos/Runner/Setup.swift). Mac only, for
// this Mac's own Claktory, never in Claktory Dev.
import 'dart:async';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'forms.dart' show FormFrame;
import 'identity.dart';
import 'lid_closed.dart' show removeLidHelper;
import 'main.dart' show isDevBuild;
import 'protocol/factory/v1/factory.pb.dart' show LidClosedState;
import 'resources.dart' show formatBytes;
import 'scope.dart';
import 'settings_view.dart' show SettingRow;
import 'setup.dart';

/// How the app reaches `factoryd uninstall`; tests set another.
SetupChannel removalChannel = const SetupChannel();

/// Whether this app can remove Claktory from this Mac: the Mac's real
/// Claktory (not Claktory Dev) showing its own. Tests set [removable].
bool canRemoveThisMac(BuildContext context) {
  final scope = AppScope.scope(context);
  return (removable ??
          (defaultTargetPlatform == TargetPlatform.macOS && !isDevBuild)) &&
      scope.backend.localBackend &&
      scope.backend.target == null;
}

/// Overrides where removing is offered, for tests.
@visibleForTesting
bool? removable;

/// Settings → This Mac's one row.
class RemoveThisMac extends StatelessWidget {
  const RemoveThisMac({super.key});

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return SettingRow(
      label: 'Remove',
      top: true,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Button(
            label: 'Remove Claktory from this Mac…',
            onPressed: () => showModal(
              context,
              (context, close) => RemoveForm(close: close),
            ),
          ),
          const SizedBox(height: Space.xs),
          Text(
            'Stops your agents and removes Claktory from this Mac. Your '
            'project folders stay.',
            style: TextStyles.caption.copyWith(color: palette.textFaint),
          ),
        ],
      ),
    );
  }
}

/// The confirmation: keep the data or delete it too, a second
/// confirmation that names what's lost when deleting, then removing.
class RemoveForm extends StatefulWidget {
  const RemoveForm({super.key, required this.close});

  final VoidCallback close;

  @override
  State<RemoveForm> createState() => _RemoveFormState();
}

class _RemoveFormState extends State<RemoveForm> {
  var _deleteData = false;

  /// On the second confirmation, for deleting the data.
  var _confirmingDelete = false;
  var _removing = false;
  String? _error;
  String _detail = '';
  var _showDetail = false;

  /// How big the data is, once the backend has said (0 until then), and
  /// whether that's all of it.
  var _dataBytes = 0;
  var _dataComplete = true;

  Future<void> _measure() async {
    final dry = await removalChannel.uninstall(const [
      'uninstall',
      '--dry-run',
      '--delete-data',
    ]);
    if (mounted && dry.dataBytes > 0) {
      setState(() {
        _dataBytes = dry.dataBytes;
        _dataComplete = dry.dataComplete;
      });
    }
  }

  Future<void> _remove() async {
    setState(() {
      _removing = true;
      _error = null;
      _detail = '';
    });
    // Keeping agents working with the lid closed goes first: the backend
    // lets go of the Mac and its helper is removed (lid-closed.md I5).
    final backend = AppScope.of(context);
    if (backend.workspace.workspace.keepAwake.lidClosed !=
        LidClosedState.LID_CLOSED_OFF) {
      try {
        await removeLidHelper(backend);
      } on Exception {
        // Removed with the app all the same: macOS drops a helper whose
        // app is gone, and the helper lets go within 10 minutes.
      }
    }
    final result = await removalChannel.uninstall([
      'uninstall',
      if (_deleteData) '--delete-data',
    ]);
    if (result.result == 'removed' || result.result == 'not_installed') {
      // Claktory is gone: so is this app.
      await removalChannel.removeApp(forget: _deleteData);
      return;
    }
    if (!mounted) return;
    setState(() {
      _removing = false;
      _error = !result.answered
          ? "This version of Claktory can't remove itself. Update Claktory, "
                'then try again.'
          : result.reason.isNotEmpty
          ? result.reason
          : "Claktory couldn't be removed.";
      _detail = result.detail;
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final failure = [
      if (_detail.trim().isNotEmpty) ...[
        Align(
          alignment: Alignment.centerLeft,
          child: Pressable(
            onTap: () => setState(() => _showDetail = !_showDetail),
            builder: (context, hovered) => Text(
              _showDetail ? 'Hide details' : 'Show details',
              style: TextStyles.caption.copyWith(
                color: hovered ? palette.text : palette.textMuted,
              ),
            ),
          ),
        ),
        if (_showDetail)
          Padding(
            padding: const EdgeInsets.only(top: Space.xs),
            child: Text(
              _detail.trim(),
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          ),
      ],
    ];
    final again = _error != null;
    if (_confirmingDelete) {
      final agents = workspace.agents.length;
      final secrets = workspace.vault.secrets.length;
      final size = _dataBytes <= 0
          ? ''
          : ' (${_dataComplete ? '' : 'at least '}'
                '${formatBytes(Int64(_dataBytes))})';
      return FormFrame(
        title: 'Delete all your Claktory data?',
        body:
            'This deletes ${_count(agents, 'agent')} with their '
            'conversations and memory, ${_count(secrets, 'secret')}, your '
            "sign-ins and settings$size. Agents' own checkouts go too, with "
            "any work not yet in your project folders. Your other devices "
            "lose access. You can't undo this. Your project folders stay.",
        error: _error,
        submitLabel: _removing
            ? 'Removing…'
            : again
            ? 'Try again'
            : 'Delete and remove',
        onSubmit: _removing ? null : _remove,
        onCancel: widget.close,
        fields: failure,
      );
    }
    return FormFrame(
      title: 'Remove Claktory from this Mac?',
      body:
          'Your agents stop and Claktory is removed. Your project folders '
          'stay.',
      error: _error,
      submitLabel: _removing
          ? 'Removing…'
          : _deleteData
          ? 'Continue…'
          : again
          ? 'Try again'
          : 'Remove Claktory',
      onSubmit: _removing
          ? null
          : _deleteData
          ? () {
              setState(() {
                _confirmingDelete = true;
                _error = null;
              });
              unawaited(_measure());
            }
          : _remove,
      onCancel: widget.close,
      fields: [
        ToggleTap(
          value: _deleteData,
          onChanged: _removing
              ? null
              : (on) => setState(() {
                  _deleteData = on;
                  _error = null;
                }),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      'Also delete your data',
                      style: TextStyles.body.copyWith(color: palette.text),
                    ),
                    Text(
                      'Your agents and their conversations, memory, secrets, '
                      "sign-ins and settings. Unless you delete it, it's kept, "
                      'so you can install Claktory again and carry on.',
                      style: TextStyles.caption.copyWith(
                        color: palette.textMuted,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: Space.m),
              Padding(
                padding: const EdgeInsets.only(top: 2),
                child: Toggle(
                  value: _deleteData,
                  label: 'Also delete your data',
                  onChanged: _removing
                      ? null
                      : (on) => setState(() {
                          _deleteData = on;
                          _error = null;
                        }),
                ),
              ),
            ],
          ),
        ),
        ...failure,
      ],
    );
  }
}

/// "1 agent", "14 agents".
String _count(int count, String thing) =>
    count == 1 ? '1 $thing' : '$count ${thing}s';
