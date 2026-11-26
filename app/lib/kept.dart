import 'package:flutter/widgets.dart';

/// Keeps the widget [builder] made while [inputs] stay the same, so a
/// change elsewhere in the Workspace doesn't build it again. A busy team
/// changes the Workspace several times a second (each agent's step, its
/// mood, its context), and every change would otherwise build every row of
/// the sidebar and every message in view again, for one agent's change.
///
/// Inputs are the same when each is identical to, or equal to, the one
/// before; a list among them is compared item by item. A Workspace patch
/// keeps what didn't change as the same objects, so an unchanged agent is
/// identical and costs nothing to compare.
///
/// [builder] must use only [inputs], what it reads through its context
/// (the palette, the text size: a change there builds it again), and what
/// never changes (callbacks over an element that outlives it). Anything else
/// it reads from the Workspace goes in [inputs], or it won't follow.
class Kept extends StatefulWidget {
  const Kept({super.key, required this.inputs, required this.builder});

  final List<Object?> inputs;
  final WidgetBuilder builder;

  @override
  State<Kept> createState() => _KeptState();
}

class _KeptState extends State<Kept> {
  Widget? _built;
  List<Object?> _builtFor = const [];

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // Something [builder] read through its context changed.
    _built = null;
  }

  @override
  Widget build(BuildContext context) {
    if (_built case final built? when sameInputs(_builtFor, widget.inputs)) {
      return built;
    }
    _builtFor = widget.inputs;
    return _built = widget.builder(context);
  }
}

/// Whether [a] and [b] hold the same inputs ([Kept]).
bool sameInputs(List<Object?> a, List<Object?> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    final was = a[i];
    final now = b[i];
    if (identical(was, now)) continue;
    if (was is List<Object?> && now is List<Object?>) {
      if (!sameInputs(was, now)) return false;
    } else if (was != now) {
      return false;
    }
  }
  return true;
}
