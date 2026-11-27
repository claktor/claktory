import 'dart:math' as math;

import 'columns.dart';

/// How Needs you's focus mode is laid out in an iPad's window: the
/// workbench's own tiers ([tierFor]), with the queue, the stage and
/// Context arranged to fit. Nothing here keeps state;
/// the session (needs_you_focus.dart) holds the card, its draft and
/// whether Context is open, so a resize only lays them out again.
enum FocusComposition {
  /// Queue, stage and Context side by side.
  columns,

  /// Queue and stage; Context opens as a panel over the stage's right.
  contextOver,

  /// The stage alone, with "3 of 7" and ‹ › in its header: the queue is a
  /// popover from the count, Context a panel over nearly all of the stage.
  stageOnly,

  /// The phone's focus mode, one card a screen, with the iPad's
  /// keys and Pencil.
  phone,
}

/// The starting sizes, in points, at the default text size.
abstract final class FocusGeometry {
  /// The queue in [FocusComposition.columns].
  static const queue = 280.0;

  /// The queue in [FocusComposition.contextOver].
  static const queueNarrow = 240.0;

  /// Context's least width beside the stage.
  static const context = 360.0;

  /// Context as a panel over the stage in [FocusComposition.contextOver],
  /// at most; never more than [contextOverShare] of the window.
  static const contextPanel = 400.0;
  static const contextOverShare = 0.6;

  /// What a panel over the stage alone leaves of it, so it reads as over.
  static const stageStrip = 48.0;

  /// The card's column on the stage, at most, and the stage's padding on
  /// each side of it.
  static const cardColumn = 720.0;
  static const stagePadding = 24.0;

  /// The queue as a popover, at most.
  static const queuePopover = 360.0;

  /// A queue row: a 28pt character, the name over what it asks.
  static const row = 64.0;
}

/// Where each part goes in a window [width] wide at [tier].
class FocusColumns {
  const FocusColumns._({
    required this.composition,
    required this.queue,
    required this.stage,
    required this.context,
    required this.contextPanel,
  });

  final FocusComposition composition;

  /// The queue's width beside the stage; 0 when it isn't there.
  final double queue;

  /// The stage's width.
  final double stage;

  /// Context's width beside the stage; 0 when it's a panel instead.
  final double context;

  /// Context's width as a panel over the stage; 0 when it's a column.
  final double contextPanel;

  /// Whether Context is a column of its own (the C key then hides it and
  /// gives its room to the stage).
  bool get contextBeside => context > 0;

  /// The stage's width with Context hidden by the person, in [columns].
  double get stageAlone => stage + context + (context > 0 ? 1 : 0);
}

/// The focus mode's columns for a window [width] wide at [tier], with the
/// person's text [textFactor] times the default ([textFactorOf]): the
/// queue widens with it, up to half again, so its names still read. One
/// hairline between each two columns. The stage takes up to its card's
/// column; what's left goes to Context, at least [FocusGeometry.context].
FocusColumns focusColumnsFor(
  WorkbenchTier tier,
  double width, {
  double textFactor = 1,
}) {
  final grow = textFactor.clamp(1.0, 1.5);
  switch (tier) {
    case WorkbenchTier.full:
      final queue = FocusGeometry.queue * grow;
      final room = width - queue - 2;
      final wanted = FocusGeometry.cardColumn + 2 * FocusGeometry.stagePadding;
      final stage = math.min(wanted, room - FocusGeometry.context);
      return FocusColumns._(
        composition: FocusComposition.columns,
        queue: queue,
        stage: stage,
        context: room - stage,
        contextPanel: 0,
      );
    case WorkbenchTier.paired:
      final queue = FocusGeometry.queueNarrow * grow;
      return FocusColumns._(
        composition: FocusComposition.contextOver,
        queue: queue,
        stage: width - queue - 1,
        context: 0,
        contextPanel: math.min(
          FocusGeometry.contextPanel,
          width * FocusGeometry.contextOverShare,
        ),
      );
    case WorkbenchTier.focused:
      return FocusColumns._(
        composition: FocusComposition.stageOnly,
        queue: 0,
        stage: width,
        context: 0,
        contextPanel: width - FocusGeometry.stageStrip,
      );
    case WorkbenchTier.compact:
      return FocusColumns._(
        composition: FocusComposition.phone,
        queue: 0,
        stage: width,
        context: 0,
        // Over the one-card screen, a strip of it left to tap back to
        // .
        contextPanel: width - FocusGeometry.stageStrip,
      );
  }
}
