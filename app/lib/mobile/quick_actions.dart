import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../inbox.dart' show needsYouCount;
import '../scope.dart';

/// The iPhone's Home Screen quick action, touch and hold the app's icon:
/// Needs you, with how many wait. A tap opens Needs you on the
/// Claktory the phone shows, on its first card.
const quickActionTitle = 'Needs you';

/// Its subtitle: how many wait, or nothing when nothing does.
String quickActionSubtitle(int count) => count == 0 ? '' : '$count waiting';

/// The channel to the app's native side (AppDelegate.swift, QuickActions).
const quickActionsChannel = MethodChannel('factory/quick_actions');

/// Keeps the quick action's subtitle as the Claktory shown says, and opens
/// Needs you when it's tapped. Only on an iPhone; elsewhere, just [child].
class PhoneQuickActions extends StatefulWidget {
  const PhoneQuickActions({super.key, required this.child});

  final Widget child;

  @override
  State<PhoneQuickActions> createState() => _PhoneQuickActionsState();
}

/// The quick action's taps' taker.
_PhoneQuickActionsState? _taking;

class _PhoneQuickActionsState extends State<PhoneQuickActions> {
  /// What the quick action says now, so it's set only when it changes.
  String? _said;

  bool get _on => defaultTargetPlatform == TargetPlatform.iOS;

  @override
  void initState() {
    super.initState();
    if (!_on) return;
    // The latest one mounted takes taps: one going (moved in the tree)
    // goes after the one that replaced it came.
    _taking = this;
    quickActionsChannel.setMethodCallHandler((call) async {
      if (call.method == 'open') _taking?._open();
    });
    unawaited(_listen());
  }

  Future<void> _listen() async {
    try {
      // Tapped before the app listened: it launched the app.
      final waiting = await quickActionsChannel.invokeMethod<bool>('listen');
      if (waiting == true) _open();
    } on MissingPluginException {
      // No native side (tests).
    } on PlatformException {
      // Nor an answer.
    }
  }

  @override
  void dispose() {
    if (_taking == this) {
      _taking = null;
      quickActionsChannel.setMethodCallHandler(null);
    }
    super.dispose();
  }

  void _open() {
    WidgetsBinding.instance
      ..addPostFrameCallback((_) {
        if (mounted) AppScope.openPage(context, AppPage.inbox);
      })
      // An app at rest draws no frame by itself.
      ..ensureVisualUpdate();
  }

  void _say(int count) {
    final subtitle = quickActionSubtitle(count);
    if (_said == subtitle) return;
    _said = subtitle;
    unawaited(() async {
      try {
        await quickActionsChannel.invokeMethod('set', {
          'title': quickActionTitle,
          'subtitle': subtitle,
        });
      } on MissingPluginException {
        // No native side (tests).
      } on PlatformException {
        // Kept as it was.
      }
    }());
  }

  @override
  Widget build(BuildContext context) {
    if (!_on) return widget.child;
    final workspace = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: workspace,
      builder: (context, child) {
        _say(needsYouCount(workspace.workspace));
        return child!;
      },
      child: widget.child,
    );
  }
}
