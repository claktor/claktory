// History's video export (#458 E1, history-replay.md "Video export"):
// "Share replay…" makes an MP4 of the replay. Nothing is uploaded by
// Claktory.
//
// The frames are drawn from the same board as the replay, each stepped to a
// fixed time (never in real time, so nothing drops), rendered at 1080
// pixels, and given to the Runner's encoder (app/macos/Runner/
// ReplayWriter.swift, the Mac's and the iPhone's). Making it runs in the
// background (export_job.dart): the person can do anything meanwhile, and
// once it's made it's saved where everyone knows to look, ~/Movies/Claktory
// on a Mac, Photos on an iPhone or iPad. Android has no encoder yet: the
// export isn't offered there.
//
// A share shows only the shape unless the person turns more on (Fugo,
// msg 33087): names, roles, project names and dates are off by default;
// secrets, apps and databases aren't in it at all, other Claktories are
// never named, and paths never shown. The file carries no metadata, and its
// header's dates are zeroed unless dates are shown.
import 'dart:async';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../map.dart' show MapGridPainter;
import '../map_layout.dart';
import '../mobile/drawer.dart' show largeText;
import '../mobile/metrics.dart' show TypeScale;
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import 'board.dart';
import 'controller.dart';
import 'export_job.dart';
import 'scene.dart';
import 'scrubber.dart' show chapterAt, historyWhen;
import 'timeline.dart';
import 'warp.dart';

/// Frames a second.
const replayFps = 30;

/// Where an export can be made: where the Runner has the encoder.
bool get canExportReplay =>
    !kIsWeb &&
    (defaultTargetPlatform == TargetPlatform.macOS ||
        defaultTargetPlatform == TargetPlatform.iOS);

/// The video's shape, and its size in pixels.
enum ReplayShape {
  square('Square', 1080, 1080),
  portrait('Portrait', 1080, 1920),
  landscape('Landscape', 1920, 1080);

  const ReplayShape(this.title, this.width, this.height);

  final String title;
  final int width;
  final int height;

  /// Its size as laid out: half its pixels, drawn at twice.
  Size get logical => Size(width / 2, height / 2);
}

/// How long the video is.
enum ReplayLength {
  half('30 seconds', 30),
  minute('1 minute', 60);

  const ReplayLength(this.title, this.seconds);

  final String title;
  final int seconds;
}

/// What a share shows beyond the shape: all off unless turned on.
@immutable
class ReplayPrivacy {
  const ReplayPrivacy({
    this.names = false,
    this.roles = false,
    this.projects = false,
    this.dates = false,
  });

  final bool names;
  final bool roles;
  final bool projects;
  final bool dates;

  ReplayPrivacy copyWith({
    bool? names,
    bool? roles,
    bool? projects,
    bool? dates,
  }) => ReplayPrivacy(
    names: names ?? this.names,
    roles: roles ?? this.roles,
    projects: projects ?? this.projects,
    dates: dates ?? this.dates,
  );
}

/// What a share calls things, as [privacy] allows.
class ShareWords implements BoardWords {
  ShareWords(this.scene, this.privacy) {
    // Projects by when they first appeared: "Project 1", "Project 2"…
    final projects = <int>[
      for (var i = 0; i < scene.boxes.length; i++)
        if (scene.boxes[i].box.kind == MapBoxKind.project) i,
    ];
    double first(int box) {
      final node = scene.boxes[box].node;
      if (node <= 0) return double.infinity;
      final spans = scene.timeline.nodeSpans[node - 1];
      return spans.isEmpty ? double.infinity : spans.first.from;
    }

    projects.sort((a, b) {
      final by = first(a).compareTo(first(b));
      return by != 0 ? by : a.compareTo(b);
    });
    final ids = <String, int>{};
    for (final box in projects) {
      ids.putIfAbsent(scene.boxes[box].box.id, () => ids.length + 1);
    }
    _projectNumber = ids;
  }

  final HistoryScene scene;
  final ReplayPrivacy privacy;
  late final Map<String, int> _projectNumber;

  @override
  (String, String) chip(int chip, String label, String detail) {
    final node = scene.chips[chip].chip.node;
    switch (node.kind) {
      case MapNodeKind.MAP_NODE_KIND_AGENT:
        if (privacy.names) return (label, privacy.roles ? detail : '');
        // A role in the name's place, or just "Agent".
        if (privacy.roles && detail.isNotEmpty) return (detail, '');
        return ('Agent', '');
      // A secret's name names a service and an account: never shown.
      case MapNodeKind.MAP_NODE_KIND_SECRET:
        return ('', '');
      // "Kai's database" names its agent.
      case MapNodeKind.MAP_NODE_KIND_DATA:
        return privacy.names ? (label, '') : ('Database', '');
      default:
        return (label, '');
    }
  }

  @override
  (String, String) box(int box) {
    final it = scene.boxes[box].box;
    // Never a path: it can name the person and what they work on.
    switch (it.kind) {
      case MapBoxKind.project:
        return privacy.projects
            ? (it.label, '')
            : ('Project ${_projectNumber[it.id] ?? ''}'.trim(), '');
      case MapBoxKind.folder:
        return privacy.projects ? (it.label, '') : ('', '');
      default:
        return (it.label, '');
    }
  }

  // A pipe's name names the other side: only with names (Bruno 44370).
  @override
  String pad(int pad) => privacy.names ? scene.pads[pad].label : '';

  // Their initials and colours would say who it is: the plain avatar
  // unless names show (Fugo).
  @override
  Person person(Person person) => privacy.names ? person : Person();
}

/// A moved thing: from where it was to where it goes, eased over [hop].
class _Place {
  _Place(this.to, this.opacity) : from = to;

  Rect from;
  Rect to;
  double since = 0;
  double opacity;
  var chip = -1;

  static const hop = 0.6;

  Rect at(double p) {
    if (from == to) return to;
    final f = ((p - since) / hop).clamp(0.0, 1.0);
    return Rect.lerp(from, to, Curves.easeInOutCubic.transform(f))!;
  }

  /// Toward [target], from wherever it is at [p]; at once while unseen.
  void go(Rect target, double p, {required bool snap}) {
    if (target == to) return;
    if (snap || opacity == 0) {
      from = to = target;
      return;
    }
    from = at(p);
    to = target;
    since = p;
  }

  /// Toward [target] opacity, [dt] seconds on; at once when [dt] is null.
  void fade(double target, double? dt) {
    if (dt == null) {
      opacity = target;
      return;
    }
    const fade = 0.3;
    opacity = target > opacity
        ? math.min(target, opacity + dt / fade)
        : math.max(target, opacity - dt / fade);
  }
}

/// The video, a frame at a time: its own playhead and warp over the
/// replay's history, so the whole of it fits [length].
class ReplayVideo {
  ReplayVideo(HistoryController replay, {required ReplayLength length})
    : timeline = replay.timeline!,
      scene = replay.scene! {
    video = HistoryController(replay.source)
      ..timeline = timeline
      ..scene = scene
      ..routes = replay.routes
      ..log = replay.log
      ..project = replay.project
      ..loaded = true;
    final end = math.max(timeline.end, replay.log.last);
    TimeWarp warp(double seconds) => TimeWarp.build(
      start: timeline.start,
      end: end,
      speed: HistorySpeed.auto,
      times: replay.log.times,
      loaded: const [(double.negativeInfinity, double.infinity)],
      seconds: seconds,
      limited: false,
    );
    // The busy part's seconds that make the whole of it [length], less a
    // last look at the end: what's squeezed plays for less as it goes
    // faster, so it's found by halving.
    final target = length.seconds - tail;
    var lo = 0.01;
    var hi = target;
    for (var k = 0; k < 40; k++) {
      final mid = (lo + hi) / 2;
      if (warp(mid).duration > target) {
        hi = mid;
      } else {
        lo = mid;
      }
    }
    video.warp = warp(lo);
    frames = ((video.warp!.duration + tail) * replayFps).ceil();
    // Messages sent by each event, so far: no task, monitor or Claktory
    // ticks.
    // With one project, only those to or from it.
    final log = replay.log;
    final project = replay.project;
    bool ours(int node, double t) => scene.nodeInProject(node, t, project!);

    bool counts(int i) {
      if (log.kind(i).tick) return false;
      if (project == null) return true;
      final t = log.time(i);
      if (ours(log.from(i), t)) return true;
      for (var k = 0; k < log.toCount(i); k++) {
        if (ours(log.to(i, k), t)) return true;
      }
      return false;
    }

    _sent = Int32List(log.length + 1);
    for (var i = 0; i < log.length; i++) {
      _sent[i + 1] = _sent[i] + (counts(i) ? 1 : 0);
    }
  }

  /// The end, held this long.
  static const tail = 2.0;

  final HistoryTimeline timeline;
  final HistoryScene scene;
  late final HistoryController video;
  late final int frames;
  late final Int32List _sent;

  /// The frame stepped to: its moment, picture, places and camera.
  var index = -1;
  double get time => video.time.value;
  SceneFrame? frame;
  BoardStill? still;
  Rect? camera;

  final _chips = <String, _Place>{};
  final _boxes = <_Place>[];
  final _pads = <_Place>[];

  /// To frame [i]: everything moves on from the frame before, or is put
  /// in place at once for the first or one before it.
  void step(int i, BoardWords words) {
    final p = i / replayFps;
    final double? dt = index < 0 || i <= index ? null : (i - index) / replayFps;
    index = i;
    final t = video.historyAt(math.min(p, video.warp!.duration));
    video.time.value = t;
    final frame = _team(scene.at(t, project: video.project));
    this.frame = frame;

    // One chip a node: a moved agent's shown one, or where it was last.
    final shownChip = <String, int>{};
    final firstChip = <String, int>{};
    for (var c = 0; c < scene.chips.length; c++) {
      final origin = scene.originOf(scene.chips[c].id);
      firstChip.putIfAbsent(origin, () => c);
      if (frame.chips[c] != hidden) shownChip[origin] = c;
    }
    for (final MapEntry(key: origin, value: first) in firstChip.entries) {
      final shown = shownChip[origin];
      final place = _chips.putIfAbsent(
        origin,
        () =>
            _Place(scene.chips[shown ?? first].rect, 0)..chip = shown ?? first,
      );
      if (shown != null) {
        place.go(scene.chips[shown].rect, p, snap: dt == null);
        place.chip = shown;
      }
      place.fade(shown != null ? 1 : 0, dt);
    }
    for (var b = 0; b < scene.boxes.length; b++) {
      final shown = frame.boxes[b] != hidden;
      if (b == _boxes.length) _boxes.add(_Place(frame.boxRects[b], 0));
      if (shown) _boxes[b].go(frame.boxRects[b], p, snap: dt == null);
      _boxes[b].fade(shown ? 1 : 0, dt);
    }
    for (var k = 0; k < scene.pads.length; k++) {
      if (k == _pads.length) _pads.add(_Place(scene.pads[k].rect, 0));
      _pads[k].fade(frame.pads[k] != hidden ? 1 : 0, dt);
    }
    still = BoardStill(
      chips: [
        for (final place in _chips.values)
          if (place.opacity > 0)
            (chip: place.chip, rect: place.at(p), opacity: place.opacity),
      ],
      boxRects: [for (final box in _boxes) box.at(p)],
      boxOpacity: [for (final box in _boxes) box.opacity],
      padOpacity: [for (final pad in _pads) pad.opacity],
      words: words,
    );
    // The camera eases toward what's there: out quickly, so what comes is
    // soon in the picture, and in slowly.
    final target = _bounds(frame) ?? Offset.zero & scene.layout.size;
    final camera = this.camera;
    if (camera == null || dt == null) {
      this.camera = target;
    } else {
      double ease(double from, double to, bool out) =>
          from + (to - from) * (1 - math.exp(-dt / (out ? 0.2 : 0.6)));
      this.camera = Rect.fromLTRB(
        ease(camera.left, target.left, target.left < camera.left),
        ease(camera.top, target.top, target.top < camera.top),
        ease(camera.right, target.right, target.right > camera.right),
        ease(camera.bottom, target.bottom, target.bottom > camera.bottom),
      );
    }
  }

  /// [frame] with only the team: the strips of secrets, apps and databases
  /// along the foot, and their lines, left out. They're as wide as there
  /// are of them, so they'd shrink the team to nothing, and what's in them
  /// would show only as blanks.
  SceneFrame _team(SceneFrame frame) {
    for (var c = 0; c < scene.chips.length; c++) {
      final kind = scene.chips[c].chip.node.kind;
      if (kind == MapNodeKind.MAP_NODE_KIND_SECRET ||
          kind == MapNodeKind.MAP_NODE_KIND_DATA ||
          kind == MapNodeKind.MAP_NODE_KIND_APP) {
        frame.chips[c] = hidden;
      }
    }
    for (var w = 0; w < scene.wires.length; w++) {
      switch (scene.wires[w].wire.layer) {
        case MapLayer.secrets || MapLayer.data || MapLayer.apps:
          frame.wires[w] = hidden;
        default:
      }
    }
    for (var b = 0; b < scene.boxes.length; b++) {
      switch (scene.boxes[b].box.kind) {
        case MapBoxKind.vault || MapBoxKind.apps:
          frame.boxes[b] = hidden;
        default:
      }
    }
    return frame;
  }

  /// What the video frames: the person, the agents, the boards and the
  /// pads.
  Rect? _bounds(SceneFrame frame) {
    Rect? out = scene.layout.you;
    void add(Rect rect) => out = out?.expandToInclude(rect) ?? rect;
    for (var c = 0; c < scene.chips.length; c++) {
      if (frame.chips[c] == hidden) continue;
      if (scene.chips[c].chip.node.kind == MapNodeKind.MAP_NODE_KIND_AGENT) {
        add(scene.chips[c].rect);
      }
    }
    for (var b = 0; b < scene.boxes.length; b++) {
      if (frame.boxes[b] == hidden) continue;
      final kind = scene.boxes[b].box.kind;
      if (kind == MapBoxKind.workspace || kind == MapBoxKind.project) {
        add(frame.boxRects[b]);
      }
    }
    for (var k = 0; k < scene.pads.length; k++) {
      if (frame.pads[k] != hidden) add(scene.pads[k].rect);
    }
    return out;
  }

  /// How many messages had been sent by now.
  int get messages => _sent[video.log.upperBound(time)];

  /// How many agents there are now, and how many of them are working.
  ({int working, int total}) get working => frame == null
      ? (working: 0, total: 0)
      : scene.agentsAt(video.log, time, project: video.project);

  /// How many agents there are now.
  int get agents => working.total;

  void dispose() => video.time.dispose();
}

/// Which day of the history [t] is in, the first (from [start]'s) day 1,
/// by the calendar here.
int replayDay(double start, double t) {
  DateTime date(double ms) {
    final at = DateTime.fromMillisecondsSinceEpoch(ms.toInt());
    return DateTime(at.year, at.month, at.day);
  }

  // By whole days, whatever a clock change did to one.
  return (date(t).difference(date(start)).inHours + 12) ~/ 24 + 1;
}

/// How many agents there are, and how many work: "4 of 23 agents
/// working", the same in History's bar and its video.
String agentsWorking(({int working, int total}) agents) {
  final (:working, :total) = agents;
  if (working == 0) return total == 1 ? '1 agent' : '${_grouped(total)} agents';
  if (total == 1) return '1 agent working';
  return '${_grouped(working)} of ${_grouped(total)} agents working';
}

/// "1,204".
String _grouped(int n) {
  final digits = '$n';
  final out = StringBuffer();
  for (var i = 0; i < digits.length; i++) {
    if (i > 0 && (digits.length - i) % 3 == 0) out.write(',');
    out.write(digits[i]);
  }
  return out.toString();
}

/// One frame of the video, at its size as laid out: the board under its
/// camera, when it is and how many, and the watermark.
class ReplayStage extends StatelessWidget {
  const ReplayStage({
    super.key,
    required this.video,
    required this.shape,
    required this.privacy,
    required this.person,
  });

  final ReplayVideo video;
  final ReplayShape shape;
  final ReplayPrivacy privacy;
  final Person person;

  /// Room kept for the words at the top and the watermark at the foot.
  static const _top = 64.0;
  static const _foot = 48.0;
  static const _side = 20.0;

  Matrix4 _fit(Rect camera, Size size) {
    final area = Rect.fromLTRB(
      _side,
      _top,
      size.width - _side,
      size.height - _foot,
    );
    final scale = math.min(
      1.0,
      math.min(
        area.width / math.max(1, camera.width),
        area.height / math.max(1, camera.height),
      ),
    );
    return Matrix4.identity()
      ..translateByDouble(
        area.center.dx - camera.center.dx * scale,
        area.center.dy - camera.center.dy * scale,
        0,
        1,
      )
      ..scaleByDouble(scale, scale, 1, 1);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final size = shape.logical;
    final frame = video.frame;
    final still = video.still;
    final camera = video.camera;
    if (frame == null || still == null || camera == null) {
      return SizedBox.fromSize(
        size: size,
        child: ColoredBox(color: palette.canvas),
      );
    }
    final transform = _fit(camera, size);
    final t = video.time;
    final day = replayDay(video.timeline.start, t);
    // One project's: only its own chapters (Abbacchio's row 1584).
    final chapter = privacy.names && privacy.projects
        ? chapterAt(video.video.chapters, t)
        : null;
    final agents = video.working;
    final messages = video.messages;
    // One project's video names it only when projects are named.
    final project = video.video.project;
    final box = project == null || !privacy.projects
        ? -1
        : video.scene.projectBox(project);
    final projectName = box < 0 ? null : video.scene.boxes[box].box.label;
    return MediaQuery(
      data: MediaQuery.of(context)
          .copyWith(textScaler: TextScaler.noScaling, disableAnimations: true),
      child: SizedBox.fromSize(
        size: size,
        child: ClipRect(
          child: Stack(
            children: [
              Positioned.fill(child: ColoredBox(color: palette.canvas)),
              Positioned.fill(
                child: CustomPaint(
                  painter: MapGridPainter(transform, palette.hairline),
                ),
              ),
              // Above the watermark's own strip: the board never runs
              // under it (Lucy).
              Positioned(
                left: 0,
                top: 0,
                width: size.width,
                height: size.height - _foot + Space.s,
                child: ClipRect(
                  child: Transform(
                    transform: transform,
                    child: OverflowBox(
                      alignment: Alignment.topLeft,
                      minWidth: video.scene.layout.size.width,
                      maxWidth: video.scene.layout.size.width,
                      minHeight: video.scene.layout.size.height,
                      maxHeight: video.scene.layout.size.height,
                      child: HistoryBoard(
                        controller: video.video,
                        frame: frame,
                        person: person,
                        still: still,
                      ),
                    ),
                  ),
                ),
              ),
              Positioned(
                left: _side,
                right: _side,
                top: Space.m,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      privacy.dates
                          ? historyWhen(context, t)
                          : 'Day ${math.max(1, day)}',
                      maxLines: 1,
                      style: TextStyles.title.copyWith(
                        color: palette.text,
                        fontFeatures: const [ui.FontFeature.tabularFigures()],
                      ),
                    ),
                    Text(
                      [
                        ?projectName,
                        agentsWorking(agents),
                        messages == 1
                            ? '1 message'
                            : '${_grouped(messages)} messages',
                        if (chapter != null) chapter.title,
                      ].join(' · '),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyles.caption.copyWith(
                        color: palette.textMuted,
                        fontFeatures: const [ui.FontFeature.tabularFigures()],
                      ),
                    ),
                  ],
                ),
              ),
              Positioned(
                right: _side,
                bottom: Space.s,
                child: const ReplayWatermark(),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Whose video it is, at its foot: Claktory's mark, then "Made with
/// Claktory · claktory.com", legible but quiet (Matt, through Bruno 44277;
/// Trish's words, Bruno 46771: whoever watches it isn't its owner).
class ReplayWatermark extends StatelessWidget {
  const ReplayWatermark({super.key});

  /// The app icon's colors: a white cog with a blue shadow on graphite.
  static const _graphite = Color(0xFF2A2D33);
  static const _blue = Color(0xFF2F6FEB);
  static const _white = Color(0xFFFFFFFF);

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final style = TextStyles.caption.copyWith(
      color: palette.textMuted,
      fontWeight: FontWeight.w500,
    );
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          width: 20,
          height: 20,
          decoration: BoxDecoration(
            color: _graphite,
            borderRadius: BorderRadius.circular(5),
          ),
          child: const Stack(
            children: [
              Positioned(
                left: 3.5,
                top: 3.5,
                width: 14,
                height: 14,
                child: FittedBox(child: CogGlyph(color: _blue)),
              ),
              Positioned(
                left: 3,
                top: 3,
                width: 14,
                height: 14,
                child: FittedBox(child: CogGlyph(color: _white)),
              ),
            ],
          ),
        ),
        const SizedBox(width: Space.s),
        Text.rich(
          TextSpan(
            children: [
              TextSpan(
                text: 'Made with Claktory',
                style: style.copyWith(color: palette.text),
              ),
              TextSpan(text: ' · claktory.com', style: style),
            ],
          ),
          maxLines: 1,
        ),
      ],
    );
  }
}

/// Opens the export sheet for [replay] (its events all in).
Future<void> showReplayExport(BuildContext context, HistoryController replay) =>
    showModal(
      context,
      (context, close) => ReplayExport(replay: replay, onClose: close),
    );

class ReplayExport extends StatefulWidget {
  const ReplayExport({super.key, required this.replay, required this.onClose});

  final HistoryController replay;
  final VoidCallback onClose;

  @override
  State<ReplayExport> createState() => _ReplayExportState();
}

class _ReplayExportState extends State<ReplayExport>
    with SingleTickerProviderStateMixin {
  var _length = ReplayLength.half;
  var _privacy = const ReplayPrivacy();

  /// Made once every event is in.
  ReplayVideo? _made;
  ReplayVideo get _video => _made!;
  late final Ticker _ticker;
  ReplayShape? _chosenShape;
  final _stage = GlobalKey();

  @override
  void initState() {
    super.initState();
    _ticker = createTicker(_tick);
    widget.replay.addListener(_loaded);
    replayJobs.addListener(_jobs);
    _loaded();
  }

  /// Once every event is in, the video can be made.
  void _loaded() {
    if (_made != null || !widget.replay.loaded) return;
    _made = ReplayVideo(widget.replay, length: _length);
    if (mounted) setState(() {});
    _ticker.start();
  }

  /// Portrait on a phone, square elsewhere, until one is chosen: the
  /// board's natural shape is wide, so the others are offered.
  ReplayShape get _shape =>
      _chosenShape ??
      (isPhoneLayout(context) ? ReplayShape.portrait : ReplayShape.square);

  /// A video being made finished, or went: this one can be made now.
  void _jobs() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    replayJobs.removeListener(_jobs);
    widget.replay.removeListener(_loaded);
    _ticker.dispose();
    _made?.dispose();
    super.dispose();
  }

  ShareWords get _words => ShareWords(_video.scene, _privacy);

  /// The preview plays the video as it will be, round and round.
  void _tick(Duration elapsed) {
    final i = (elapsed.inMicroseconds * replayFps ~/ 1000000) % _video.frames;
    if (i == _video.index) return;
    setState(() => _video.step(i, _words));
  }

  void _restart({ReplayLength? length}) {
    _made?.dispose();
    _made = ReplayVideo(widget.replay, length: length ?? _length);
    _ticker
      ..stop()
      ..start();
  }

  /// Starts making the video in the background, from its first frame,
  /// and puts the sheet away: its progress, then where it's saved, show
  /// along the top of the window (export_job.dart).
  void _make() {
    replayJobs.start(
      ReplayJob(
        video: ReplayVideo(widget.replay, length: _length),
        shape: _shape,
        privacy: _privacy,
        person: AppScope.of(context).workspace.workspace.person,
        media: MediaQuery.of(context),
        style: DefaultTextStyle.of(context).style,
      ),
    );
    widget.onClose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final phone = isPhoneLayout(context);
    // One video at a time: while one is made, its progress shows along the
    // top of the window, and another waits for it.
    final busy = replayJobs.busy;
    if (_made == null) {
      return FormFrame(
        title: 'Share replay',
        body: 'Loading the history…',
        fields: const [],
        error: null,
        submitLabel: 'Make video',
        onSubmit: null,
        onCancel: widget.onClose,
      );
    }
    final person = AppScope.of(context).workspace.workspace.person;
    final logical = _shape.logical;
    final preview = Center(
      child: ConstrainedBox(
        constraints: BoxConstraints(
          maxWidth: phone ? double.infinity : 360,
          maxHeight: phone ? 360 : 360,
        ),
        child: AspectRatio(
          aspectRatio: logical.width / logical.height,
          child: DecoratedBox(
            decoration: BoxDecoration(
              border: Border.all(color: palette.hairline),
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(Radii.control),
              child: FittedBox(
                child: RepaintBoundary(
                  key: _stage,
                  child: ReplayStage(
                    video: _video,
                    shape: _shape,
                    privacy: _privacy,
                    person: person,
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
    Widget toggle(String label, bool value, ValueChanged<bool> onChanged) =>
        Row(
          children: [
            Expanded(
              child: Text(
                label,
                style: type.body.copyWith(color: palette.text),
              ),
            ),
            const SizedBox(width: Space.m),
            Toggle(value: value, onChanged: onChanged, label: label),
          ],
        );
    Widget choice<T>(
      String label,
      T value,
      List<(T, String)> options,
      ValueChanged<T> onChanged,
    ) {
      final name = Text(label, style: type.body.copyWith(color: palette.text));
      final menu = Select<T>(
        value: value,
        options: [
          for (final (option, title) in options) SelectOption(option, title),
        ],
        onChanged: onChanged,
      );
      // At the largest text sizes the menu under its name, the width of
      // the sheet, so its choice reads whole (Zucchero's row 512).
      if (largeText(context)) {
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            name,
            const SizedBox(height: Space.xs),
            menu,
          ],
        );
      }
      return Row(
        children: [
          Expanded(child: name),
          const SizedBox(width: Space.m),
          SizedBox(width: 160, child: menu),
        ],
      );
    }

    final fields = <Widget>[
      preview,
      choice<ReplayShape>('Shape', _shape, [
        for (final shape in ReplayShape.values) (shape, shape.title),
      ], (shape) => setState(() => _chosenShape = shape)),
      choice<ReplayLength>(
        'Length',
        _length,
        [for (final length in ReplayLength.values) (length, length.title)],
        (length) => setState(() {
          _length = length;
          _restart(length: length);
        }),
      ),
      Text(
        'Only the shape of your team shows unless you turn more on. '
        'Secrets and the names of other Claktories never show.',
        style: type.caption.copyWith(color: palette.textMuted),
      ),
      Column(
        spacing: phone ? 0 : Space.s,
        children: [
          toggle(
            'Names',
            _privacy.names,
            (on) => setState(() => _privacy = _privacy.copyWith(names: on)),
          ),
          toggle(
            'Roles',
            _privacy.roles,
            (on) => setState(() => _privacy = _privacy.copyWith(roles: on)),
          ),
          toggle(
            'Project names',
            _privacy.projects,
            (on) => setState(() => _privacy = _privacy.copyWith(projects: on)),
          ),
          toggle(
            'Dates',
            _privacy.dates,
            (on) => setState(() => _privacy = _privacy.copyWith(dates: on)),
          ),
        ],
      ),
      if (!busy)
        Text(
          defaultTargetPlatform == TargetPlatform.macOS
              ? "It's made in the background and saved in $replayFolderName."
              : "It's made in the background and saved in Photos.",
          style: type.caption.copyWith(color: palette.textMuted),
        ),
    ];
    return FormFrame(
      title: 'Share replay',
      // Why Make video is greyed, at the top, where it shows without
      // scrolling on the smallest phone (Zucchero's row 662).
      body: busy
          ? "Another video is being made. You can make this one once it's "
                'done.'
          : null,
      fields: fields,
      error: null,
      submitLabel: 'Make video',
      onSubmit: busy ? null : _make,
      onCancel: widget.onClose,
    );
  }
}

/// Zeroes the dates an MP4's header carries (when it was made and last
/// changed, in the movie's, each track's and each media's header), so the
/// file doesn't say when; the rest stays as it is.
Future<void> clearMp4Dates(File file) async {
  final raf = await file.open(mode: FileMode.append);
  try {
    final length = await raf.length();
    var at = 0;
    while (at + 8 <= length) {
      await raf.setPosition(at);
      final head = ByteData.sublistView(await raf.read(16));
      var size = head.getUint32(0);
      final type = String.fromCharCodes(head.buffer.asUint8List(4, 4));
      if (size == 1) size = head.getUint64(8);
      if (size == 0) size = length - at;
      if (size < 8) return;
      if (type == 'moov') {
        await raf.setPosition(at);
        final moov = await raf.read(size);
        clearMp4DatesIn(moov);
        await raf.setPosition(at);
        await raf.writeFrom(moov);
      }
      at += size;
    }
  } finally {
    await raf.close();
  }
}

/// [clearMp4Dates] on a box's bytes, in place.
@visibleForTesting
void clearMp4DatesIn(Uint8List box) {
  final data = ByteData.sublistView(box);
  void walk(int from, int end) {
    var at = from;
    while (at + 8 <= end) {
      var size = data.getUint32(at);
      final type = String.fromCharCodes(box.sublist(at + 4, at + 8));
      var header = 8;
      if (size == 1 && at + 16 <= end) {
        size = data.getUint64(at + 8);
        header = 16;
      }
      if (size == 0) size = end - at;
      if (size < header || at + size > end) return;
      switch (type) {
        case 'moov' || 'trak' || 'mdia':
          walk(at + header, at + size);
        case 'mvhd' || 'tkhd' || 'mdhd':
          final version = box[at + header];
          final dates = at + header + 4;
          final width = version == 1 ? 16 : 8;
          if (dates + width <= at + size) {
            box.fillRange(dates, dates + width, 0);
          }
      }
      at += size;
    }
  }

  walk(0, box.length);
}
