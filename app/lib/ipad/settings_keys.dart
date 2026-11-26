// Settings on an iPad's workbench, for the system's menu (Lucy's Settings
// spec §4.1, §7): while Settings is the page shown or the pane being read,
// ⌘F is "Search Settings" in the menu (hold ⌘) and puts the keys in its
// search field. ↑/↓ and Esc stay Settings' own keys in its index: a key
// command on a bare arrow or Esc would take it from every text field.
import 'package:flutter/widgets.dart';

import 'workbench.dart' show WorkbenchFocus, WorkbenchScope;

/// Puts the keys in Settings' search field: settings_view.dart handles it,
/// importing it from here, so there's one class.
class FocusSettingsSearchIntent extends Intent {
  const FocusSettingsSearchIntent();
}

/// Settings' body, inside the Actions that handle its intents: while it's
/// mounted, the iPad's workbench (if there's one above) knows Settings is
/// shown and invokes [FocusSettingsSearchIntent] on its context for ⌘F.
/// Nothing on a Mac or a phone, where there's no workbench.
class WorkbenchSettings extends StatefulWidget {
  const WorkbenchSettings({super.key, required this.child});

  final Widget child;

  @override
  State<WorkbenchSettings> createState() => _WorkbenchSettingsState();
}

class _WorkbenchSettingsState extends State<WorkbenchSettings> {
  WorkbenchFocus? _focus;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_focus != null) return;
    // Not depending on it: the workbench's changes needn't rebuild Settings.
    _focus = context.getInheritedWidgetOfExactType<WorkbenchScope>()?.notifier
      ?..showSettings(context);
  }

  @override
  void dispose() {
    _focus?.hideSettings(context);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
