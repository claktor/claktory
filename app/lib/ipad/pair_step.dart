// An iPad with no Claktory yet, at any width: the phone's way of
// connecting (PairScreen: what Claktory needs, Send me the link, signing in
// with an email, scanning or pasting a code), never the Mac's ConnectStep,
// which only pastes. A narrow window takes the phone's layout and shows
// PairScreen already; this is the wide one's, beside the rail.
import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import '../identity.dart' show Palette;
import '../mobile/metrics.dart' show withoutInsets;
import '../phone.dart' show PairScreen;

/// The pairing screen's key, the same in the narrow layout's (phone.dart)
/// and the wide one's: crossing between them (Split View and full screen,
/// a rotation from 504 to 809) builds a new window (main.dart), and the
/// key carries the screen across, at its step, rather than starting over
/// . Only one is ever there at a time.
final pairScreenKey = GlobalKey(debugLabel: 'pairing');

/// [PairScreen] filling the window beside the rail (Shell's steps place
/// it), on the canvas, inside the safe area and above the keyboard.
class IPadPairStep extends StatelessWidget {
  const IPadPairStep({super.key});

  @override
  Widget build(BuildContext context) {
    final safe = MediaQuery.paddingOf(context);
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    return ColoredBox(
      color: Palette.of(context).canvas,
      child: Padding(
        // The rail keeps the left edge's safe area.
        padding: EdgeInsets.only(
          right: safe.right,
          top: safe.top,
          bottom: math.max(safe.bottom, keyboard),
        ),
        child: withoutInsets(context, child: PairScreen(key: pairScreenKey)),
      ),
    );
  }
}
