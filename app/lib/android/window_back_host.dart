import 'package:flutter/widgets.dart';

import '../mobile/back.dart';
import '../mobile/platform_policy.dart';

/// One Android dispatcher above the whole window Overlay, after AppZoom.
///
/// Mount once, with the selected Claktory as [window]. Do not key this widget
/// by that Claktory: its registry survives a switch, but its going window's
/// registrations do not. The phone drawer borrows this registry instead of
/// mounting another dispatcher. Non-Android paths must keep their old tree.
class AndroidWindowBackHost extends StatefulWidget {
  const AndroidWindowBackHost({
    super.key,
    required this.window,
    required this.child,
  });

  final Object window;
  final Widget child;

  @override
  State<AndroidWindowBackHost> createState() => _AndroidWindowBackHostState();
}

class _AndroidWindowBackHostState extends State<AndroidWindowBackHost> {
  final _back = PhoneBack.root();

  @override
  void dispose() {
    _back.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    // Size alone does not retire overlays: an in-progress form must survive
    // a resize. Only selecting another Claktory replaces this window lease.
    final visible = _back.branch(widget.window);
    return PhonePlatformPolicy.of(context).backHost(
      back: _back,
      child: PhoneBackScope(back: visible, child: widget.child),
    );
  }
}
