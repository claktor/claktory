// Pull down to refresh a list, the platform's own way.
import 'dart:async';

import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoSliverRefreshControl;
import 'package:flutter/widgets.dart';
import 'package:material_ui/material_ui.dart' show RefreshIndicator;

import '../identity.dart';
import 'haptics.dart';
import 'platform_policy.dart';

/// [list] with a pull down past its top to [onRefresh], as the platform
/// does it ([PhonePlatformPolicy.pull]): on an iPhone the list bounces
/// down over a spinner held until [onRefresh] completes, a tap felt as it
/// lets go; on Android a circle drops in over the list's top. [list] gets
/// the physics its scroll view must use, which let it be pulled however
/// short it is, and the slivers that go first in it. With a mouse there's
/// no pull: [list] as it is, the physics left to the platform.
class PullToRefresh extends StatelessWidget {
  const PullToRefresh({super.key, required this.onRefresh, required this.list})
    : pulls = true;

  /// [list] pulled to refresh only where [pulls]; else as it is.
  const PullToRefresh.when(
    this.pulls, {
    super.key,
    required this.onRefresh,
    required this.list,
  });

  final bool pulls;
  final Future<void> Function() onRefresh;
  final Widget Function(ScrollPhysics? physics, List<Widget> first) list;

  @override
  Widget build(BuildContext context) => switch (pulls
      ? PhonePlatformPolicy.of(context).pull
      : PullStyle.none) {
    PullStyle.none => list(null, const []),
    PullStyle.iPhone => list(
      const AlwaysScrollableScrollPhysics(parent: BouncingScrollPhysics()),
      [
        CupertinoSliverRefreshControl(
          onRefresh: () {
            unawaited(Haptics.light());
            return onRefresh();
          },
        ),
      ],
    ),
    PullStyle.android => RefreshIndicator(
      onRefresh: onRefresh,
      color: Palette.of(context).signal,
      backgroundColor: Palette.of(context).sidebar,
      child: list(
        const AlwaysScrollableScrollPhysics(parent: ClampingScrollPhysics()),
        const [],
      ),
    ),
  };
}
