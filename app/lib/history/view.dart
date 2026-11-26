// History replay (#458): the Map's History mode. The team growing from day
// one on the Map's own board, its lines appearing over time and every
// message pulsing along them, with a scrubber to go anywhere.
//
// On a Mac or iPad it fills the Map's pane under the key: the board, then
// the scrubber and the controls. On a phone it fills the Map's page, the
// controls in a bar at its foot and the chapters in a sheet.
import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../canvas.dart';
import '../controls.dart';
import '../identity.dart';
import '../map.dart' show MapGridPainter;
import '../map_card.dart' show MapCardPlacement;
import '../map_layout.dart';
import '../mobile/drawer.dart' show largeText;
import '../mobile/metrics.dart';
import '../mobile/presentation.dart' show SheetAction, showActions, showPanel;
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import '../selection.dart' show Unselectable;
import 'board.dart';
import 'controller.dart';
import 'export.dart';
import 'pulses.dart';
import 'scene.dart';
import 'scrubber.dart';
import 'timeline.dart';
import 'warp.dart';

/// The Map's two modes: how things are now, or how they came to be. Two
/// quiet pills in the Map's key; on a phone, iOS's segmented control in
/// the page's header, the chosen one raised on the canvas's color.
class MapModeSwitch extends StatelessWidget {
  const MapModeSwitch({
    super.key,
    required this.history,
    required this.onChanged,
    this.phone = false,
  });

  final bool history;
  final ValueChanged<bool> onChanged;
  final bool phone;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    Widget mode(String label, bool value) {
      final on = history == value;
      return Semantics(
        selected: on,
        button: true,
        inMutuallyExclusiveGroup: true,
        label: label,
        excludeSemantics: true,
        onTap: () => onChanged(value),
        child: Pressable(
          onTap: () => onChanged(value),
          builder: (context, hovered) => phone
              ? _segment(context, label, on, hovered)
              : AnimatedContainer(
                  duration: Motion.fast,
                  // At least 28, more as its words grow (row 517).
                  constraints: const BoxConstraints(minHeight: 28),
                  alignment: Alignment.center,
                  padding: const EdgeInsets.symmetric(horizontal: Space.s),
                  decoration: BoxDecoration(
                    color: on
                        ? palette.selected
                        : hovered
                        ? palette.hover
                        : null,
                    borderRadius: BorderRadius.circular(Radii.control),
                  ),
                  child: Text(
                    label,
                    style: TextStyles.caption.copyWith(
                      color: on
                          ? palette.text
                          : hovered
                          ? palette.textMuted
                          : palette.textFaint,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ),
        ),
      );
    }

    if (phone) {
      // In the header's middle, between Back and the edge.
      return Container(
        width: math.min(240.0, MediaQuery.sizeOf(context).width - 140),
        decoration: BoxDecoration(
          color: palette.hover,
          borderRadius: BorderRadius.circular(Radii.panel),
        ),
        child: IntrinsicHeight(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(child: mode('Now', false)),
              Expanded(child: mode('History', true)),
            ],
          ),
        ),
      );
    }
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        mode('Now', false),
        const SizedBox(width: Space.xs),
        mode('History', true),
      ],
    );
  }

  Widget _segment(BuildContext context, String label, bool on, bool pressed) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    const inset = 2.0;
    return Padding(
      padding: const EdgeInsets.all(inset),
      child: AnimatedContainer(
        duration: Motion.fast,
        curve: Curves.easeOut,
        constraints: const BoxConstraints(minHeight: 32 - inset * 2),
        padding: const EdgeInsets.symmetric(horizontal: Space.s),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: on ? palette.canvas : const Color(0x00000000),
          borderRadius: BorderRadius.circular(Radii.panel - inset),
          boxShadow: on ? floatingShadow : null,
        ),
        // Shrunk to its segment rather than cut, as iOS's segmented
        // controls do, where a narrow header meets the largest text (Sale's
        // row 149).
        child: FittedBox(
          fit: BoxFit.scaleDown,
          child: Text(
            label,
            maxLines: 1,
            style: type.secondary.copyWith(
              fontWeight: on ? FontWeight.w600 : FontWeight.w500,
              color: on || pressed ? palette.text : palette.textMuted,
            ),
          ),
        ),
      ),
    );
  }
}

/// Opens the video export for the History [key] shows (a [HistoryView]'s
/// key): from a phone's header.
void shareReplay(BuildContext context, GlobalKey key) {
  final state = key.currentState;
  if (state is! _HistoryViewState) return;
  final controller = state._controller;
  if (controller == null || controller.timeline == null) return;
  unawaited(showReplayExport(context, controller));
}

class HistoryView extends StatefulWidget {
  const HistoryView({super.key, this.phone = false});

  /// A phone's page: the controls in a bar at its foot.
  final bool phone;

  @override
  State<HistoryView> createState() => _HistoryViewState();
}

class _HistoryViewState extends State<HistoryView>
    with SingleTickerProviderStateMixin {
  HistoryController? _controller;
  late final Ticker _ticker = createTicker(_tick);
  final _canvas = CanvasController();
  SceneFrame? _frame;
  var _last = Duration.zero;

  /// The camera follows what's on the board until the person pans or
  /// zooms; Follow takes it back.
  var _follow = true;
  Rect? _camera;
  var _framing = false;

  /// Where the camera's rect was on screen once it was shown: a canvas
  /// that's moved from there was moved by the person.
  Rect? _shownAt;

  /// The canvas has been laid out once, so it can be moved.
  var _ready = false;
  var _settled = true;

  int? _hover;
  int? _pinned;
  int? _card;
  Timer? _cardWait;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_controller != null) return;
    final backend = AppScope.of(context);
    final controller = HistoryController(
      BackendHistory(backend),
      order: sidebarOrder(backend.workspace.workspace),
      measure: _measure,
    );
    _controller = controller;
    controller.addListener(_changed);
    controller.time.addListener(_moved);
    _canvas.addListener(_canvasMoved);
    unawaited(controller.open());
  }

  @override
  void dispose() {
    _cardWait?.cancel();
    _ticker.dispose();
    _canvas.removeListener(_canvasMoved);
    _canvas.dispose();
    _controller?.removeListener(_changed);
    _controller?.dispose();
    super.dispose();
  }

  static double _measure(String text) {
    final painter = TextPainter(
      text: TextSpan(
        text: text,
        style: TextStyles.caption.copyWith(fontWeight: FontWeight.w500),
      ),
      textDirection: TextDirection.ltr,
      maxLines: 1,
    )..layout();
    final width = painter.width;
    painter.dispose();
    return width.ceilToDouble();
  }

  HistoryController get controller => _controller!;

  void _changed() {
    if (!mounted) return;
    setState(() {});
    _moved();
  }

  /// The playhead moved: what's on the board then, and the camera after
  /// it.
  void _moved() {
    final scene = controller.scene;
    if (scene == null || !mounted) return;
    final frame = scene.at(controller.time.value, project: controller.project);
    if (!frame.sameAs(_frame)) {
      setState(() => _frame = frame);
      _settled = false;
    }
    _run();
  }

  void _run() {
    final needed =
        (controller.playing && !controller.idle) || (_follow && !_settled);
    if (needed && !_ticker.isActive) {
      _last = Duration.zero;
      unawaited(_ticker.start());
    } else if (!needed && _ticker.isActive) {
      _ticker.stop();
    }
  }

  void _tick(Duration elapsed) {
    final seconds = _last == Duration.zero
        ? 0.0
        : (elapsed - _last).inMicroseconds / 1e6;
    _last = elapsed;
    // A long frame (the window hidden, a breakpoint) doesn't jump ahead.
    controller.advance(math.min(seconds, 0.1));
    _frameCamera(seconds);
    _run();
  }

  /// Eases the camera toward what's on the board, over about a second.
  void _frameCamera(double seconds) {
    final scene = controller.scene;
    final frame = _frame;
    if (!_follow || scene == null || frame == null) {
      _settled = true;
      return;
    }
    final bounds = scene.bounds(frame);
    if (bounds == null || !_ready) return;
    final target = bounds.inflate(mapCell * 3);
    final reduced = MediaQuery.disableAnimationsOf(context);
    final from = _camera ?? target;
    final k = reduced || seconds == 0 && _camera == null
        ? 1.0
        : 1 - math.exp(-seconds / 0.3);
    var next = Rect.lerp(from, target, k)!;
    final close =
        (next.left - target.left).abs() < 0.5 &&
        (next.top - target.top).abs() < 0.5 &&
        (next.right - target.right).abs() < 0.5 &&
        (next.bottom - target.bottom).abs() < 0.5;
    if (close) next = target;
    _settled = close;
    _camera = next;
    _framing = true;
    _canvas.show(next);
    _shownAt = _canvas.toView(next);
    _framing = false;
  }

  void _canvasMoved() {
    // The person panned or zoomed: the camera stays where they put it.
    final camera = _camera;
    final shown = _shownAt;
    if (_framing || !_follow || camera == null || shown == null) return;
    final now = _canvas.toView(camera);
    if ((now.left - shown.left).abs() < 1 &&
        (now.top - shown.top).abs() < 1 &&
        (now.width - shown.width).abs() < 1) {
      return;
    }
    setState(() => _follow = false);
  }

  void _followAgain() {
    setState(() => _follow = true);
    _settled = false;
    _run();
  }

  void _hovered(int? chip) {
    _cardWait?.cancel();
    setState(() {
      _hover = chip;
      if (chip != _card) _card = null;
    });
    if (chip == null) return;
    _cardWait = Timer(const Duration(milliseconds: 300), () {
      if (mounted && _hover == chip) setState(() => _card = chip);
    });
  }

  void _tapped(int chip) =>
      setState(() => _pinned = _pinned == chip ? null : chip);

  /// Space, arrows, Home and End, 1 to 5 for the speeds, J and K for the
  /// chapters.
  Map<ShortcutActivator, VoidCallback> get _keys {
    void step(double fraction) {
      final timeline = controller.timeline;
      if (timeline == null) return;
      final span = controller.last - timeline.start;
      controller.seek(controller.time.value + span * fraction);
    }

    return {
      const SingleActivator(LogicalKeyboardKey.space): controller.toggle,
      const SingleActivator(LogicalKeyboardKey.arrowRight): () => step(0.02),
      const SingleActivator(LogicalKeyboardKey.arrowLeft): () => step(-0.02),
      const SingleActivator(LogicalKeyboardKey.arrowRight, shift: true): () =>
          step(0.1),
      const SingleActivator(LogicalKeyboardKey.arrowLeft, shift: true): () =>
          step(-0.1),
      const SingleActivator(LogicalKeyboardKey.home): () {
        final timeline = controller.timeline;
        if (timeline != null) controller.seek(timeline.start);
      },
      const SingleActivator(LogicalKeyboardKey.end): controller.goLive,
      const SingleActivator(LogicalKeyboardKey.keyJ): () =>
          controller.chapter(forward: false),
      const SingleActivator(LogicalKeyboardKey.keyK): () =>
          controller.chapter(forward: true),
      for (final (i, key) in [
        LogicalKeyboardKey.digit1,
        LogicalKeyboardKey.digit2,
        LogicalKeyboardKey.digit3,
        LogicalKeyboardKey.digit4,
        LogicalKeyboardKey.digit5,
      ].indexed)
        SingleActivator(key): () => controller.setSpeed(HistorySpeed.values[i]),
    };
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = widget.phone;
    final scene = controller.scene;
    final frame = _frame;
    final problem = controller.problem;
    final body = TypeScale.of(context).body;
    if (scene == null || frame == null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(Space.xl),
          child: problem == null
              ? Text(
                  'Loading the history…',
                  style: body.copyWith(color: palette.textFaint),
                )
              : Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      problem,
                      textAlign: TextAlign.center,
                      style: body.copyWith(color: palette.textMuted),
                    ),
                    if (!problem.startsWith('Update')) ...[
                      const SizedBox(height: Space.m),
                      Button(label: 'Try again', onPressed: controller.retry),
                    ],
                  ],
                ),
        ),
      );
    }
    if (!_ready) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted) return;
        _ready = true;
        _settled = false;
        _run();
      });
    }
    final person = AppScope.of(context).workspace.workspace.person;
    final focus = _hover ?? _pinned;
    final cardChip = phone ? _pinned : (_card ?? _pinned);
    return CallbackShortcuts(
      bindings: _keys,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(
            child: Stack(
              children: [
                Positioned.fill(
                  child: MediaQuery.withNoTextScaling(
                    child: ZoomCanvas(
                      controller: _canvas,
                      size: scene.layout.size,
                      background: (transform) =>
                          MapGridPainter(transform, palette.hairline),
                      child: HistoryBoard(
                        controller: controller,
                        frame: frame,
                        person: person,
                        focus: focus,
                        onHover: phone ? (_) {} : _hovered,
                        onTap: _tapped,
                        budget: phone ? phonePulseBudget : pulseBudget,
                      ),
                    ),
                  ),
                ),
                Positioned(
                  top: Space.s,
                  left: 0,
                  right: 0,
                  child: IgnorePointer(child: _Stamp(controller: controller)),
                ),
                if (!_follow)
                  Positioned(
                    right: phone ? Touch.gutter : Space.m,
                    top: Space.s,
                    child: Button(label: 'Follow', onPressed: _followAgain),
                  ),
                if (cardChip != null && frame.chips[cardChip] != hidden)
                  _cardFor(cardChip, phone: phone),
              ],
            ),
          ),
          if (phone)
            _PhoneBar(controller: controller)
          else
            _Controls(controller: controller),
        ],
      ),
    );
  }

  Widget _cardFor(int chip, {required bool phone}) {
    final card = _HistoryCard(controller: controller, chip: chip);
    if (phone) {
      return Positioned(
        left: Space.s,
        right: Space.s,
        bottom: Space.s,
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: _HistoryCard(
              controller: controller,
              chip: chip,
              width: null,
              onClose: () => setState(() => _pinned = null),
            ),
          ),
        ),
      );
    }
    final rect = controller.scene!.chips[chip].rect;
    return Positioned.fill(
      child: IgnorePointer(
        child: ListenableBuilder(
          listenable: _canvas,
          builder: (context, _) => CustomSingleChildLayout(
            delegate: MapCardPlacement(_canvas.toView(rect)),
            child: card,
          ),
        ),
      ),
    );
  }
}

/// "8 h later", for a moment, as playing skips a quiet stretch.
class _Stamp extends StatelessWidget {
  const _Stamp({required this.controller});

  final HistoryController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ValueListenableBuilder<double>(
      valueListenable: controller.time,
      builder: (context, t, _) {
        final warp = controller.warp;
        final gap = warp?.gapBefore(t);
        final since = gap == null
            ? double.infinity
            : controller.playAt(t) - controller.playAt(gap.to);
        final show = controller.playing && gap != null && since < 1.0;
        return AnimatedOpacity(
          duration: Motion.base,
          opacity: show ? 1 : 0,
          child: Center(
            child: Text(
              gap == null ? '' : '${laterText(gap.length)} later',
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textMuted),
            ),
          ),
        );
      },
    );
  }
}

/// How long a quiet stretch was: "25 min", "8 h", "3 days".
String laterText(double ms) {
  final minutes = ms / Duration.millisecondsPerMinute;
  if (minutes < 60) return '${minutes.round()} min';
  final hours = minutes / 60;
  if (hours < 48) return '${hours.round()} h';
  return '${(hours / 24).round()} days';
}

/// The scrubber and, under it, play, when it is, the chapter, the speed
/// and Now.
class _Controls extends StatelessWidget {
  const _Controls({required this.controller});

  final HistoryController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Unselectable(
      child: Container(
        padding: const EdgeInsets.fromLTRB(Space.m, Space.xs, Space.m, Space.s),
        decoration: BoxDecoration(
          border: Border(top: BorderSide(color: palette.hairline)),
        ),
        child: LayoutBuilder(
          builder: (context, constraints) => ListenableBuilder(
            listenable: controller,
            builder: (context, _) =>
                largeText(context) ||
                    constraints.maxWidth < _Controls.rowWidth(context)
                ? _stacked(context)
                : Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      HistoryScrubber(controller: controller),
                      const SizedBox(height: Space.xs),
                      // One row: when it is takes what's left, so the
                      // speed, Now and Share replay… stay put as the
                      // chapter's words change (Sale's row 149).
                      Row(
                        children: [
                          _PlayButton(controller: controller),
                          // Before the chapter's words, whose length changes, so
                          // they stay put under repeated taps (Zucchero's row 498).
                          GlyphButton(
                            label: 'Previous chapter',
                            active: false,
                            onTap: () => controller.chapter(forward: false),
                            glyph: (color) => ExtensionGlyph(
                              icon: 'chevron-left',
                              name: '',
                              color: color,
                            ),
                          ),
                          GlyphButton(
                            label: 'Next chapter',
                            active: false,
                            onTap: () => controller.chapter(forward: true),
                            glyph: (color) => ExtensionGlyph(
                              icon: 'chevron-right',
                              name: '',
                              color: color,
                            ),
                          ),
                          const SizedBox(width: Space.s),
                          Expanded(
                            // On a second line when it needs one.
                            child: _When(controller: controller, lines: 2),
                          ),
                          if (_hasProjects(controller)) ...[
                            const SizedBox(width: Space.s),
                            SizedBox(
                              width: 180,
                              child: _ProjectSelect(controller: controller),
                            ),
                          ],
                          const SizedBox(width: Space.s),
                          SizedBox(
                            width: 210,
                            child: Select<HistorySpeed>(
                              value: controller.speed,
                              title: 'Speed',
                              options: [
                                for (final speed in HistorySpeed.values)
                                  SelectOption(speed, speed.title),
                              ],
                              onChanged: controller.setSpeed,
                            ),
                          ),
                          const SizedBox(width: Space.s),
                          _NowButton(controller: controller),
                          if (canExportReplay) ...[
                            const SizedBox(width: Space.s),
                            _share(),
                          ],
                        ],
                      ),
                    ],
                  ),
          ),
        ),
      ),
    );
  }

  /// How wide the controls' one row must be for when it is to read whole
  /// beside play, the speed, Now and Share replay…: the glyphs, the speed
  /// and the gaps (about 360 pt), then Now's and Share replay…'s words and
  /// at least 260 pt for the time and chapter on two lines, which grow with
  /// the text (Sale's row 150: at 755 pt and Large, 125 pt was left and
  /// the chapter was cut). Narrower, they stack.
  static double rowWidth(BuildContext context) =>
      360 + MediaQuery.textScalerOf(context).scale(540) + 188;

  Widget _share() => Builder(
    builder: (context) => Button(
      label: 'Share replay…',
      onPressed: controller.timeline == null
          ? null
          : () => showReplayExport(context, controller),
    ),
  );

  /// At the largest text sizes, or where the row doesn't fit (an iPad's
  /// pane, say): when it is on lines of its own, then the buttons and the
  /// speed, as wide as there's room, so neither the time, the chapter nor
  /// the speed is cut (Zucchero's row 508, Sale's row 149); the text grows
  /// only to twice, so the board keeps its room.
  Widget _stacked(BuildContext context) => MediaQuery.withClampedTextScaling(
    maxScaleFactor: 2,
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        HistoryScrubber(controller: controller),
        const SizedBox(height: Space.xs),
        _When(controller: controller, stacked: true, lines: 3),
        const SizedBox(height: Space.xs),
        Row(
          children: [
            _PlayButton(controller: controller),
            GlyphButton(
              label: 'Previous chapter',
              active: false,
              onTap: () => controller.chapter(forward: false),
              glyph: (color) =>
                  ExtensionGlyph(icon: 'chevron-left', name: '', color: color),
            ),
            GlyphButton(
              label: 'Next chapter',
              active: false,
              onTap: () => controller.chapter(forward: true),
              glyph: (color) =>
                  ExtensionGlyph(icon: 'chevron-right', name: '', color: color),
            ),
            const SizedBox(width: Space.s),
            Expanded(
              child: Select<HistorySpeed>(
                value: controller.speed,
                title: 'Speed',
                options: [
                  for (final speed in HistorySpeed.values)
                    SelectOption(speed, speed.title),
                ],
                onChanged: controller.setSpeed,
              ),
            ),
          ],
        ),
        const SizedBox(height: Space.xs),
        Wrap(
          spacing: Space.s,
          runSpacing: Space.xs,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            _NowButton(controller: controller),
            if (canExportReplay) _share(),
            if (_hasProjects(controller))
              SizedBox(
                width: 220,
                child: _ProjectSelect(controller: controller),
              ),
          ],
        ),
      ],
    ),
  );
}

/// Whether History has a project to show on its own.
bool _hasProjects(HistoryController controller) =>
    controller.scene?.projects.isNotEmpty ?? false;

/// The one project History shows, or all of them (Bruno 44372): to watch,
/// scrub and share one project's history.
class _ProjectSelect extends StatelessWidget {
  const _ProjectSelect({required this.controller});

  final HistoryController controller;

  @override
  Widget build(BuildContext context) => Select<String>(
    value: controller.project ?? '',
    title: 'Project',
    options: [
      const SelectOption('', 'All projects'),
      for (final project in controller.scene?.projects ?? const [])
        SelectOption(project.id, project.label),
    ],
    onChanged: (id) => controller.showProject(id.isEmpty ? null : id),
  );
}

/// On a phone: which project it shows, from a sheet.
class _ProjectButton extends StatelessWidget {
  const _ProjectButton({required this.controller, required this.size});

  final HistoryController controller;
  final double size;

  @override
  Widget build(BuildContext context) => GlyphButton(
    label: 'Project',
    active: controller.project != null,
    onTap: () async {
      final current = controller.project ?? '';
      // The one shown checked.
      Widget Function(Color)? mark(String id) => id == current
          ? (color) => ExtensionGlyph(icon: 'check', name: '', color: color)
          : null;
      final chosen = await showActions<String>(
        context,
        title: 'Project',
        actions: [
          SheetAction('All projects', '', glyph: mark('')),
          for (final project in controller.scene?.projects ?? const [])
            SheetAction(project.label, project.id, glyph: mark(project.id)),
        ],
      );
      if (chosen != null) {
        controller.showProject(chosen.isEmpty ? null : chosen);
      }
    },
    // In signal while it shows one project (Lucy).
    glyph: (color) => ExtensionGlyph(
      icon: 'list-filter',
      name: '',
      color: controller.project != null ? Palette.of(context).signal : color,
      size: size,
    ),
  );
}

class _PlayButton extends StatelessWidget {
  const _PlayButton({required this.controller, this.size = 16});

  final HistoryController controller;
  final double size;

  @override
  Widget build(BuildContext context) {
    final playing = controller.playing;
    return GlyphButton(
      label: playing ? 'Pause' : 'Play',
      active: false,
      onTap: controller.toggle,
      glyph: (color) => ExtensionGlyph(
        icon: playing ? 'pause' : 'play',
        name: '',
        color: color,
        size: size,
      ),
    );
  }
}

/// When the playhead is, and the chapter it's in: on a line, wrapping to
/// at most [lines], or on a phone ([stacked]) the chapter under the time,
/// each in at most [lines].
class _When extends StatelessWidget {
  const _When({required this.controller, this.stacked = false, this.lines = 1});

  final HistoryController controller;
  final bool stacked;
  final int lines;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return ListenableBuilder(
      listenable: Listenable.merge([controller, controller.time]),
      builder: (context, _) {
        final t = controller.time.value;
        final chapter = chapterAt(controller.chapters, t);
        final timeline = controller.timeline;
        final reconstructed =
            timeline != null &&
            timeline.reconstructed &&
            t < timeline.recordingSince;
        final when = TextSpan(
          text: controller.live
              ? 'Now'
              : historyWhen(context, t, short: stacked),
          style: type.label.copyWith(
            color: palette.text,
            fontFeatures: const [FontFeature.tabularFigures()],
          ),
        );
        final agents = controller.scene?.agentsAt(
          controller.log,
          t,
          project: controller.project,
        );
        final working = agents == null || agents.total == 0
            ? null
            : agentsWorking(agents);
        final more = controller.fetching || chapter != null;
        // The one project shown, first (Lucy).
        final project = controller.project;
        final box = project == null || controller.scene == null
            ? -1
            : controller.scene!.projectBox(project);
        final about = [
          if (box >= 0)
            TextSpan(
              text:
                  '${controller.scene!.boxes[box].box.label}'
                  '${reconstructed || working != null || more ? ' · ' : ''}',
              style: type.caption.copyWith(
                color: palette.text,
                fontWeight: FontWeight.w500,
              ),
            ),
          // First, so a narrow line never cuts it off (Zucchero's row 502).
          if (reconstructed)
            TextSpan(
              text: working == null && !more
                  ? 'Pieced together'
                  : 'Pieced together · ',
              style: type.caption.copyWith(color: palette.textFaint),
            ),
          // As the video says it (Matt, through Bruno 44277).
          if (working != null)
            TextSpan(
              text: more ? '$working · ' : working,
              style: type.caption.copyWith(
                color: palette.textMuted,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          // Going where the events aren't in yet: quietly, in place of
          // the chapter.
          if (controller.fetching)
            TextSpan(
              text: 'Loading…',
              style: type.caption.copyWith(color: palette.textFaint),
            )
          else if (chapter != null)
            TextSpan(
              text: chapter.title,
              style: type.caption.copyWith(color: palette.textMuted),
            ),
        ];
        if (stacked) {
          return Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text.rich(when, maxLines: lines, overflow: TextOverflow.ellipsis),
              if (about.isNotEmpty)
                Text.rich(
                  TextSpan(children: about),
                  maxLines: lines,
                  overflow: TextOverflow.ellipsis,
                ),
            ],
          );
        }
        return Text.rich(
          TextSpan(
            children: [
              when,
              if (about.isNotEmpty) const TextSpan(text: '  '),
              ...about,
            ],
          ),
          maxLines: lines,
          overflow: TextOverflow.ellipsis,
        );
      },
    );
  }
}

/// While it follows what happens now, a live dot and "Now"; else a button
/// to go there.
class _NowButton extends StatelessWidget {
  const _NowButton({required this.controller});

  final HistoryController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    if (controller.live && controller.playing) {
      return Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          LiveDot(color: palette.signal),
          const SizedBox(width: Space.xs),
          Text(
            'Now',
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textMuted),
          ),
        ],
      );
    }
    return Button(label: 'Go to Now', onPressed: controller.goLive);
  }
}

/// A phone's controls at the foot of the page: the scrubber, then play,
/// when it is, the speed and Chapters. At the largest text sizes when it
/// is gets a line of its own, above play, the speed and Chapters, so
/// neither the time nor the chapter is cut to a word (Zucchero's row 508);
/// the bar's text grows only so far, as iOS's bars, so the Map keeps its
/// room.
class _PhoneBar extends StatelessWidget {
  const _PhoneBar({required this.controller});

  final HistoryController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final bottom = MediaQuery.paddingOf(context).bottom;
    return Container(
      padding: EdgeInsets.fromLTRB(
        Touch.gutter,
        Space.xs,
        Touch.gutter,
        math.max(bottom, Space.s),
      ),
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border(top: BorderSide(color: palette.hairline)),
      ),
      child: MediaQuery.withClampedTextScaling(
        maxScaleFactor: 2,
        child: ListenableBuilder(
          listenable: controller,
          builder: (context, _) {
            final large = largeText(context);
            final glyph = large ? 28.0 : 22.0;
            final speedWidth = large ? 168.0 : 104.0;
            // What's left of one row for when it is, beside play, the
            // speed and Chapters.
            final room =
                MediaQuery.sizeOf(context).width -
                2 * Touch.gutter -
                (_hasProjects(controller) ? 3 : 2) * Touch.target -
                speedWidth -
                2 * Space.xs;
            // Stacked only where it's narrow: turned, a phone has the room
            // for one row, and the board keeps its height (row 525); else
            // where the time wouldn't read whole beside them (Sale's row
            // 149, an iPad's narrow window).
            final stacked =
                (large && MediaQuery.sizeOf(context).width < 600) ||
                room < MediaQuery.textScalerOf(context).scale(140);
            final play = _PlayButton(controller: controller, size: glyph);
            final speed = Select<HistorySpeed>(
              value: controller.speed,
              title: 'Speed',
              options: [
                for (final speed in HistorySpeed.values)
                  SelectOption(speed, speed.short),
              ],
              onChanged: controller.setSpeed,
            );
            final chapters = GlyphButton(
              label: 'Chapters',
              active: false,
              onTap: () => _openChapters(context),
              glyph: (color) => ExtensionGlyph(
                icon: 'book-open',
                name: '',
                color: color,
                size: glyph,
              ),
            );
            final project = _hasProjects(controller)
                ? _ProjectButton(controller: controller, size: glyph)
                : null;
            return Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                HistoryScrubber(
                  controller: controller,
                  height: Touch.target,
                  phone: true,
                ),
                if (stacked) ...[
                  _When(controller: controller, stacked: true, lines: 3),
                  const SizedBox(height: Space.xs),
                  Row(
                    children: [
                      play,
                      const SizedBox(width: Space.s),
                      Expanded(child: speed),
                      const SizedBox(width: Space.s),
                      ?project,
                      chapters,
                    ],
                  ),
                ] else
                  Row(
                    children: [
                      play,
                      const SizedBox(width: Space.xs),
                      Expanded(
                        // The chapter wraps rather than being cut (Sale's
                        // row 149).
                        child: _When(
                          controller: controller,
                          stacked: true,
                          lines: 2,
                        ),
                      ),
                      const SizedBox(width: Space.xs),
                      SizedBox(width: speedWidth, child: speed),
                      ?project,
                      chapters,
                    ],
                  ),
              ],
            );
          },
        ),
      ),
    );
  }

  void _openChapters(BuildContext context) => showPanel(
    context,
    title: 'Chapters',
    builder: (context, close) => _ChapterList(
      controller: controller,
      onPick: (chapter) {
        controller.seek(chapter.atMs.toDouble());
        close();
      },
    ),
  );
}

/// The chapters, to jump to: each one's title and when.
class _ChapterList extends StatelessWidget {
  const _ChapterList({required this.controller, required this.onPick});

  final HistoryController controller;
  final ValueChanged<HistoryChapter> onPick;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final chapters = controller.chapters;
    if (chapters.isEmpty) {
      return Padding(
        padding: const EdgeInsets.all(Touch.gutter),
        child: Text(
          'No chapters yet.',
          style: type.body.copyWith(color: palette.textFaint),
        ),
      );
    }
    final current = chapterAt(chapters, controller.time.value);
    return ListView(
      shrinkWrap: true,
      children: [
        for (final chapter in chapters)
          Semantics(
            button: true,
            selected: chapter == current,
            child: Pressable(
              onTap: () => onPick(chapter),
              builder: (context, hovered) => Container(
                constraints: const BoxConstraints(minHeight: Touch.row),
                padding: const EdgeInsets.symmetric(
                  horizontal: Touch.gutter,
                  vertical: Space.s,
                ),
                color: chapter == current ? palette.selected : null,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    Text(
                      chapter.title,
                      style: type.body.copyWith(color: palette.text),
                    ),
                    Text(
                      historyWhen(context, chapter.atMs.toDouble()),
                      style: type.caption.copyWith(color: palette.textMuted),
                    ),
                  ],
                ),
              ),
            ),
          ),
      ],
    );
  }
}

/// A chip's card: who it was then and how much it talked by the
/// playhead. On a phone along the bottom, with Close.
class _HistoryCard extends StatefulWidget {
  const _HistoryCard({
    required this.controller,
    required this.chip,
    this.width = 280,
    this.onClose,
  });

  final HistoryController controller;
  final int chip;
  final double? width;
  final VoidCallback? onClose;

  @override
  State<_HistoryCard> createState() => _HistoryCardState();
}

class _HistoryCardState extends State<_HistoryCard> {
  Timer? _again;

  @override
  void initState() {
    super.initState();
    // Its counts move with the playhead, a few times a second.
    _again = Timer.periodic(const Duration(milliseconds: 250), (_) {
      if (mounted && widget.controller.playing) setState(() {});
    });
  }

  @override
  void dispose() {
    _again?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final controller = widget.controller;
    final scene = controller.scene!;
    final timeline = controller.timeline!;
    final chip = scene.chips[widget.chip];
    final t = controller.time.value;
    final node = chip.node;
    final caption = type.caption.copyWith(color: palette.textMuted);
    final faint = type.caption.copyWith(color: palette.textFaint);
    final agent = chip.chip.node.kind == MapNodeKind.MAP_NODE_KIND_AGENT;
    final label = node > 0
        ? timeline.labelAt(node, t)
        : (
            label: chip.chip.node.label,
            detail: chip.chip.node.detail,
            parentId: '',
            projectId: '',
          );
    final counts = agent && node > 0 ? controller.log.countsFor(node, t) : null;
    final partner =
        counts == null ||
            counts.partner < 0 ||
            counts.partner > timeline.nodes.length
        ? null
        : counts.partner == 0
        ? 'You'
        : timeline.labelAt(counts.partner, t).label;
    final span = node > 0 ? spanAt(timeline.nodeSpans[node - 1], t) : null;
    final onClose = widget.onClose;
    return Container(
      width: widget.width,
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
        boxShadow: floatingShadow,
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              if (agent) ...[
                Dot(color: agentColor(chip.chip.node.color), size: 8),
                const SizedBox(width: Space.s),
              ],
              Expanded(
                child: Text(
                  label.label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: type.label.copyWith(
                    color: palette.text,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          if (label.detail.isNotEmpty) Text(label.detail, style: caption),
          if (counts != null) ...[
            const SizedBox(height: Space.s),
            Text(
              'Sent ${counts.sent} · Received ${counts.received}',
              style: caption.copyWith(
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
            if (partner != null) Text('Most with $partner', style: caption),
            if (controller.log.running(node, t))
              Text('Working', style: caption),
          ],
          if (span != null && span.inferred && t < timeline.recordingSince)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: Text(
                'Pieced together from what Claktory kept before it recorded '
                'its history.',
                style: faint,
              ),
            ),
          if (onClose != null)
            Padding(
              padding: const EdgeInsets.only(top: Space.m),
              child: Align(
                alignment: Alignment.centerRight,
                child: Button(label: 'Close', onPressed: onClose),
              ),
            ),
        ],
      ),
    );
  }
}
