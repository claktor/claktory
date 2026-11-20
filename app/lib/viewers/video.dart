import 'dart:async';
import 'dart:io';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../annotations.dart';
import '../backend.dart';
import '../controls.dart';
import '../identity.dart';
import '../pane_chrome.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../resources.dart' show formatBytes;
import '../scope.dart';
import '../selection.dart';
import 'registry.dart';

/// How much of a video one read brings (the backend's cap).
const videoChunk = 8 * 1024 * 1024;

/// What the player is doing, as the viewer shows it.
@immutable
class PlaybackState {
  const PlaybackState({
    this.ready = false,
    this.playing = false,
    this.position = 0,
    this.duration = 0,
    this.size = Size.zero,
    this.error,
  });

  final bool ready;
  final bool playing;

  /// Seconds.
  final double position;
  final double duration;

  /// The frame's size in pixels.
  final Size size;
  final String? error;
}

/// A player for a video file on this device: the system's (AVPlayer on
/// the Mac and iPhone), or a stand-in in tests.
abstract class VideoPlayback {
  ValueListenable<PlaybackState> get state;
  Future<void> open(String file);
  Future<void> play();
  Future<void> pause();
  Future<void> seek(double seconds);
  Widget view();
  Future<void> dispose();
}

/// Makes the player a video viewer uses.
@visibleForTesting
VideoPlayback Function() newVideoPlayback = _SystemPlayback.new;

/// The system's player, AVPlayer, through the app's own channel
/// (app/macos/Runner/VideoPlayers.swift, built for the iPhone too): each
/// video is a texture its frames are drawn into, and the player sends its
/// state whenever it plays, pauses, moves or ends.
class _SystemPlayback implements VideoPlayback {
  static const _channel = MethodChannel('claktory/video');
  static final _open = <int, _SystemPlayback>{};

  static Future<Object?> _handle(MethodCall call) async {
    if (call.method != 'state') return null;
    final state = Map<Object?, Object?>.from(call.arguments as Map);
    _open[(state['texture'] as num).toInt()]?._update(state);
    return null;
  }

  int? _texture;
  int _quarterTurns = 0;
  bool _disposed = false;
  final _state = ValueNotifier(const PlaybackState());

  @override
  ValueListenable<PlaybackState> get state => _state;

  void _update(Map<Object?, Object?> state) {
    if (_disposed) return;
    double number(String key) => (state[key] as num?)?.toDouble() ?? 0;
    _quarterTurns = (state['quarterTurns'] as num?)?.toInt() ?? 0;
    final width = number('width'), height = number('height');
    _state.value = PlaybackState(
      ready: true,
      playing: state['playing'] == true,
      position: number('position'),
      duration: number('duration'),
      size: _quarterTurns.isOdd ? Size(height, width) : Size(width, height),
    );
  }

  @override
  Future<void> open(String file) async {
    _channel.setMethodCallHandler(_handle);
    try {
      final answer = await _channel.invokeMapMethod<Object?, Object?>('open', {
        'path': file,
      });
      final texture = (answer?['texture'] as num?)?.toInt();
      if (texture == null) {
        _state.value = const PlaybackState(
          error: "Claktory can't play this video.",
        );
        return;
      }
      if (_disposed) {
        await _channel.invokeMethod('close', {'texture': texture});
        return;
      }
      _texture = texture;
      _open[texture] = this;
      _update(answer!);
    } on PlatformException catch (error) {
      _state.value = PlaybackState(error: error.message ?? error.code);
    } on MissingPluginException {
      _state.value = const PlaybackState(
        error: "Claktory can't play this video.",
      );
    }
  }

  Future<void> _call(String method, [Map<String, Object?>? arguments]) async {
    final texture = _texture;
    if (texture == null) return;
    await _channel.invokeMethod(method, {'texture': texture, ...?arguments});
  }

  @override
  Future<void> play() => _call('play');

  @override
  Future<void> pause() => _call('pause');

  @override
  Future<void> seek(double seconds) => _call('seek', {'seconds': seconds});

  @override
  Widget view() {
    final texture = _texture;
    if (texture == null) return const SizedBox.shrink();
    return RotatedBox(
      quarterTurns: _quarterTurns,
      child: Texture(textureId: texture),
    );
  }

  @override
  Future<void> dispose() async {
    _disposed = true;
    final texture = _texture;
    _texture = null;
    _state.dispose();
    if (texture == null) return;
    _open.remove(texture);
    await _channel.invokeMethod('close', {'texture': texture});
  }
}

/// A video: brought from the machine Claktory runs on a chunk at a time
/// (with how far it got), then played by the system's player, fitted to
/// the pane over a timeline: play and pause, the time, and a track to
/// scrub. Comments are on moments and stretches of it, and on parts of the
/// frame: the header's Comment, then a click or a drag along the timeline
/// (and a drag across the frame for a part of it), marks one. Each
/// comment shows on the timeline in its agent's color; a click on one
/// plays from there.
class VideoViewer extends StatefulWidget {
  const VideoViewer({super.key, required this.path});

  final String path;

  @override
  State<VideoViewer> createState() => _VideoViewerState();
}

class _VideoViewerState extends State<VideoViewer>
    with FileLoading<VideoViewer> {
  @override
  String get path => widget.path;

  VideoPlayback? _playback;

  /// The copy on this device being played, deleted when the pane closes.
  File? _copy;

  /// How much of it has come, while it comes.
  int _received = 0;
  int _size = 0;
  String? _problem;

  /// Brings the video again whenever asked to reload.
  int _generation = 0;

  CommentsController? _wired;

  /// Marking a comment's moment (the timeline) and part of the frame.
  var _marking = false;
  Offset? _dragFrom;
  Offset? _dragTo;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final comments = this.comments;
    if (comments != _wired) {
      _wired?.removeListener(_commentsChanged);
      _wired = comments?..addListener(_commentsChanged);
      comments?.revealMoment = _play;
    }
  }

  @override
  void dispose() {
    _wired?.removeListener(_commentsChanged);
    _close();
    super.dispose();
  }

  void _close() {
    unawaited(_playback?.dispose());
    _playback = null;
    final copy = _copy;
    _copy = null;
    if (copy != null) unawaited(copy.delete().catchError((_) => copy));
  }

  /// Writing the comment ends marking.
  void _commentsChanged() {
    if (_marking && comments?.composing != null) {
      setState(() => _marking = false);
    }
  }

  @override
  Future<void> reload() async {
    final generation = ++_generation;
    final backend = AppScope.of(context);
    setState(() {
      _problem = null;
      _received = 0;
    });
    try {
      final first = await backend.readFile(path, maxBytes: videoChunk);
      if (!mounted || generation != _generation) return;
      if (first.error.isNotEmpty) {
        setState(() => file = first);
        return;
      }
      _size = first.size.toInt();
      final name = path.split('/').last;
      final version = first.sha256.isEmpty
          ? '${DateTime.now().microsecondsSinceEpoch}'
          : first.sha256.substring(0, first.sha256.length.clamp(0, 16));
      final copy = File('${Directory.systemTemp.path}/claktory-$version-$name');
      final sink = copy.openWrite();
      try {
        sink.add(first.data);
        var offset = first.data.length;
        setState(() => _received = offset);
        while (offset < _size) {
          final chunk = await backend.readFile(
            path,
            maxBytes: videoChunk,
            offset: offset,
          );
          if (!mounted || generation != _generation) return;
          if (chunk.error.isNotEmpty) return _fail(chunk.error);
          if (chunk.offset.toInt() != offset || chunk.data.isEmpty) {
            // A backend that can't read from the middle of a file.
            final name = AppScope.scope(context).connections.current?.name
                .trim();
            return _fail(
              name == null || name.isEmpty
                  ? "To play videos here, update the Claktory you're connected to."
                  : 'To play videos here, update Claktory on $name.',
            );
          }
          if (chunk.sha256 != first.sha256) {
            // Changed while it came: start again with the new one.
            unawaited(reload());
            return;
          }
          sink.add(chunk.data);
          offset += chunk.data.length;
          setState(() => _received = offset);
        }
      } finally {
        await sink.close();
      }
      if (!mounted || generation != _generation) {
        unawaited(copy.delete().catchError((_) => copy));
        return;
      }
      final playback = newVideoPlayback();
      final was = _playback?.state.value.position;
      _close();
      _copy = copy;
      _playback = playback;
      setState(() {
        file = FileContent(
          path: first.path,
          mime: first.mime,
          size: first.size,
          modifiedMs: first.modifiedMs,
          sha256: first.sha256,
        );
        failure = null;
      });
      playback.state.addListener(_playbackChanged);
      await playback.open(copy.path);
      if (was != null && was > 0) await playback.seek(was);
      if (!mounted) return;
      final comments = this.comments;
      if (comments != null && first.sha256.isNotEmpty) {
        comments.show(kind: 'video', text: '', version: first.sha256);
      }
    } on RequestFailure catch (error) {
      if (mounted) setState(() => failure = error.message);
    } on FileSystemException catch (error) {
      _fail("Couldn't load the video: ${error.message}");
    }
  }

  void _fail(String problem) {
    if (mounted) setState(() => _problem = problem);
  }

  void _playbackChanged() {
    if (mounted) setState(() {});
  }

  PlaybackState get _state => _playback?.state.value ?? const PlaybackState();

  /// Plays from [moment] (a comment picked), paused there.
  void _play(Moment moment) {
    final playback = _playback;
    if (playback == null) return;
    unawaited(playback.pause().then((_) => playback.seek(moment.start)));
  }

  void _togglePlaying() {
    final playback = _playback;
    if (playback == null) return;
    if (_state.playing) {
      unawaited(playback.pause());
    } else {
      unawaited(playback.play());
    }
  }

  void _toggleMarking() {
    final comments = this.comments;
    setState(() {
      _marking = !_marking;
      _dragFrom = _dragTo = null;
    });
    if (_marking) {
      unawaited(_playback?.pause());
      // To begin with, the moment it's paused at.
      comments?.selection.value = Moment(_state.position);
    } else if (comments?.selection.value is Moment) {
      comments?.selection.value = null;
    }
  }

  /// The person marked [seconds], or the stretch to [until].
  void _markTime(double seconds, {double? until}) {
    final comments = this.comments;
    if (comments == null) return;
    final now = comments.selection.value;
    final region = now is Moment ? now.region : null;
    final start = until == null || seconds <= until ? seconds : until;
    final end = until == null ? null : (seconds <= until ? until : seconds);
    comments.selection.value = Moment(
      start,
      end: end != null && end - start >= 0.2 ? end : null,
      region: region,
    );
  }

  /// The person dragged across the frame: the part of it the comment is on.
  void _markRegion(Rect region) {
    final comments = this.comments;
    if (comments == null) return;
    final now = comments.selection.value;
    comments.selection.value = Moment(
      now is Moment ? now.start : _state.position,
      end: now is Moment ? now.end : null,
      region: Region(region).isEmpty ? null : region,
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final file = this.file;
    final state = _state;
    final comments = file != null && file.sha256.isNotEmpty && state.ready
        ? this.comments
        : null;
    final details = this.details(
      [
        if (state.ready && state.size != Size.zero)
          '${state.size.width.round()} × ${state.size.height.round()}',
        if (state.ready) formatTime(state.duration),
        if (file != null && file.error.isEmpty) formatBytes(file.size),
      ].join(' · '),
    );
    final controls = <Widget>[
      if (comments != null) ...[
        CommentsButton(controller: comments),
        HeaderTextButton('Comment', _toggleMarking, active: _marking),
      ],
      reloadButton(),
    ];
    final problem =
        (_problem == null ? null : ViewerProblem(_problem!, path: path)) ??
        (state.error == null
            ? null
            : ViewerProblem("Claktory can't play this video.", path: path)) ??
        (file != null && file.error.isNotEmpty ? waiting() : null) ??
        (failure != null ? waiting() : null);
    final Widget body;
    if (problem != null) {
      body = problem;
    } else if (!state.ready) {
      body = _Fetching(received: _received, size: _size);
    } else {
      body = Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(child: _frame(palette, state, comments)),
          _Controls(
            state: state,
            comments: comments,
            marking: _marking,
            onPlay: _togglePlaying,
            onSeek: (seconds) => _playback?.seek(seconds),
            onMark: _markTime,
          ),
        ],
      );
    }
    return PaneScaffold(
      details: details,
      controls: controls,
      body: CallbackShortcuts(
        bindings: {
          const SingleActivator(LogicalKeyboardKey.space): _togglePlaying,
          const SingleActivator(LogicalKeyboardKey.escape): () {
            if (_marking) _toggleMarking();
          },
        },
        child: Focus(
          autofocus: false,
          child: ClipRect(
            child: ColoredBox(
              color: palette.canvas,
              child: comments == null
                  ? body
                  : Commented(controller: comments, child: body),
            ),
          ),
        ),
      ),
    );
  }

  /// The frame, fitted to the room left, with the parts of it comments are
  /// on at this moment outlined, and while marking, a drag across it marks
  /// one.
  Widget _frame(
    Palette palette,
    PlaybackState state,
    CommentsController? comments,
  ) {
    final size = state.size == Size.zero ? const Size(16, 9) : state.size;
    return Padding(
      padding: const EdgeInsets.all(Space.l),
      child: Center(
        child: AspectRatio(
          aspectRatio: size.width / size.height,
          child: LayoutBuilder(
            builder: (context, constraints) {
              final box = constraints.biggest;
              Offset fraction(Offset local) => Offset(
                (local.dx / box.width).clamp(0.0, 1.0),
                (local.dy / box.height).clamp(0.0, 1.0),
              );
              return Stack(
                fit: StackFit.expand,
                children: [
                  GestureDetector(
                    onTap: _marking ? null : _togglePlaying,
                    child: _playback?.view() ?? const SizedBox.shrink(),
                  ),
                  if (comments != null)
                    IgnorePointer(
                      child: CustomPaint(
                        painter: _FrameRegions(
                          comments: comments,
                          at: state.position,
                          drag: _dragFrom == null || _dragTo == null
                              ? null
                              : Rect.fromPoints(_dragFrom!, _dragTo!),
                          signal: palette.signal,
                          colorOf: (agentId) =>
                              _agentColor(comments.backend, agentId),
                        ),
                      ),
                    ),
                  if (_marking)
                    MouseRegion(
                      cursor: SystemMouseCursors.precise,
                      child: GestureDetector(
                        behavior: HitTestBehavior.opaque,
                        dragStartBehavior: DragStartBehavior.down,
                        onPanStart: (details) => setState(() {
                          _dragFrom = fraction(details.localPosition);
                          _dragTo = _dragFrom;
                        }),
                        onPanUpdate: (details) => setState(
                          () => _dragTo = fraction(details.localPosition),
                        ),
                        onPanEnd: (_) {
                          final from = _dragFrom;
                          final to = _dragTo;
                          setState(() => _dragFrom = _dragTo = null);
                          if (from != null && to != null) {
                            _markRegion(Rect.fromPoints(from, to));
                          }
                        },
                      ),
                    ),
                ],
              );
            },
          ),
        ),
      ),
    );
  }
}

Color _agentColor(Backend backend, String agentId) {
  for (final agent in backend.workspace.workspace.agents) {
    if (agent.id == agentId) return agentColor(agent.color);
  }
  return agentColor('');
}

/// While the video comes: how much of it has.
class _Fetching extends StatelessWidget {
  const _Fetching({required this.received, required this.size});

  final int received;
  final int size;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    if (size == 0) return const SizedBox.shrink();
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            'Loading video… ${formatBytes(Int64(received))} of '
            '${formatBytes(Int64(size))}',
            style: TextStyles.caption.copyWith(
              color: palette.textMuted,
              fontFeatures: const [FontFeature.tabularFigures()],
            ),
          ),
          const SizedBox(height: Space.s),
          SizedBox(
            width: 160,
            height: 4,
            child: ClipRRect(
              borderRadius: BorderRadius.circular(2),
              child: Stack(
                children: [
                  Positioned.fill(child: ColoredBox(color: palette.selected)),
                  FractionallySizedBox(
                    widthFactor: (received / size).clamp(0.0, 1.0),
                    child: ColoredBox(color: palette.signal),
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Under the frame, behind a hairline: play or pause, the time, and the
/// timeline.
class _Controls extends StatelessWidget {
  const _Controls({
    required this.state,
    required this.comments,
    required this.marking,
    required this.onPlay,
    required this.onSeek,
    required this.onMark,
  });

  final PlaybackState state;
  final CommentsController? comments;
  final bool marking;
  final VoidCallback onPlay;
  final void Function(double seconds) onSeek;
  final void Function(double seconds, {double? until}) onMark;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Unselectable(
      child: Container(
        padding: const EdgeInsets.fromLTRB(
          Space.s,
          Space.xs,
          Space.m,
          Space.xs,
        ),
        decoration: BoxDecoration(
          border: Border(top: BorderSide(color: palette.hairline)),
        ),
        child: Row(
          children: [
            GlyphButton(
              label: state.playing ? 'Pause' : 'Play',
              active: false,
              onTap: onPlay,
              glyph: (color) => ExtensionGlyph(
                icon: state.playing ? 'pause' : 'play',
                name: state.playing ? 'Pause' : 'Play',
                color: color,
              ),
            ),
            const SizedBox(width: Space.xs),
            Text(
              '${formatTime(state.position)} / ${formatTime(state.duration)}',
              style: TextStyles.caption.copyWith(
                color: palette.textMuted,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
            const SizedBox(width: Space.m),
            Expanded(
              child: Timeline(
                state: state,
                comments: comments,
                marking: marking,
                onSeek: onSeek,
                onMark: onMark,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// The video's length as a track: what's played filled, the playhead, and
/// each comment's moment (a dot) or stretch (a bar) above it in its agent's
/// color, the picked one stronger; the person's mark in signal. A click or
/// drag scrubs; on a comment, it picks it. While marking, a click marks a
/// moment and a drag a stretch.
class Timeline extends StatefulWidget {
  const Timeline({
    super.key,
    required this.state,
    required this.comments,
    required this.marking,
    required this.onSeek,
    required this.onMark,
  });

  final PlaybackState state;
  final CommentsController? comments;
  final bool marking;
  final void Function(double seconds) onSeek;
  final void Function(double seconds, {double? until}) onMark;

  /// A lane per row of comments that would overlap, above the track.
  static const lane = 8.0;
  static const track = 4.0;

  @override
  State<Timeline> createState() => _TimelineState();
}

class _TimelineState extends State<Timeline> {
  double? _dragFrom;

  double _seconds(Offset local, double width) {
    final duration = widget.state.duration;
    if (width <= 0 || duration <= 0) return 0;
    return (local.dx / width).clamp(0.0, 1.0) * duration;
  }

  /// The comment whose mark is under [local], if any.
  String? _hit(Offset local, Size size, List<_Placed> placed) {
    for (final mark in placed.reversed) {
      if (mark.rect(size, widget.state.duration).inflate(4).contains(local)) {
        return mark.threadId;
      }
    }
    return null;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final comments = widget.comments;
    return ListenableBuilder(
      listenable: Listenable.merge([?comments, comments?.selection]),
      builder: (context, _) {
        final placed = _place(comments);
        final lanes = placed.isEmpty
            ? 0
            : placed.map((m) => m.lane).reduce((a, b) => a > b ? a : b) + 1;
        final height = 16 + lanes * Timeline.lane;
        return LayoutBuilder(
          builder: (context, constraints) {
            final size = Size(constraints.maxWidth, height);
            return MouseRegion(
              cursor: widget.marking
                  ? SystemMouseCursors.precise
                  : SystemMouseCursors.click,
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                dragStartBehavior: DragStartBehavior.down,
                onTapUp: (details) {
                  final at = _seconds(details.localPosition, size.width);
                  if (widget.marking) {
                    widget.onMark(at);
                    return;
                  }
                  final hit = _hit(details.localPosition, size, placed);
                  if (hit != null && comments != null) {
                    comments.focus(hit);
                  } else {
                    widget.onSeek(at);
                  }
                },
                onHorizontalDragStart: (details) {
                  final at = _seconds(details.localPosition, size.width);
                  _dragFrom = at;
                  if (widget.marking) {
                    widget.onMark(at);
                  } else {
                    widget.onSeek(at);
                  }
                },
                onHorizontalDragUpdate: (details) {
                  final at = _seconds(details.localPosition, size.width);
                  if (widget.marking) {
                    widget.onMark(_dragFrom ?? at, until: at);
                  } else {
                    widget.onSeek(at);
                  }
                },
                onHorizontalDragEnd: (_) => _dragFrom = null,
                child: SizedBox(
                  height: height,
                  child: CustomPaint(
                    size: size,
                    painter: _TimelinePainter(
                      state: widget.state,
                      placed: placed,
                      selection: comments?.selection.value is Moment
                          ? comments!.selection.value! as Moment
                          : null,
                      focused: comments?.focused,
                      track: palette.selected,
                      played: palette.textMuted,
                      head: palette.text,
                      signal: palette.signal,
                      colorOf: (agentId) => comments == null
                          ? palette.textMuted
                          : _agentColor(comments.backend, agentId),
                    ),
                  ),
                ),
              ),
            );
          },
        );
      },
    );
  }

  /// The comments shown, each in the first lane where it overlaps nothing.
  List<_Placed> _place(CommentsController? comments) {
    if (comments == null) return const [];
    final placed = <_Placed>[];
    final ends = <double>[];
    final duration = widget.state.duration;
    final pad = duration <= 0 ? 0.0 : duration * 0.01;
    for (final thread in comments.inOrder(comments.shown)) {
      final moment = comments.moments[thread.id];
      if (moment == null) continue;
      final end = (moment.end ?? moment.start) + pad;
      var lane = ends.indexWhere((last) => last < moment.start);
      if (lane == -1) {
        lane = ends.length;
        ends.add(end);
      } else {
        ends[lane] = end;
      }
      placed.add(_Placed(thread.id, thread.agentId, moment, lane));
    }
    return placed;
  }
}

class _Placed {
  const _Placed(this.threadId, this.agentId, this.moment, this.lane);

  final String threadId;
  final String agentId;
  final Moment moment;
  final int lane;

  /// Where it's drawn: a bar for a stretch, a dot for a moment.
  Rect rect(Size size, double duration) {
    if (duration <= 0) return Rect.zero;
    double x(double seconds) =>
        (seconds / duration).clamp(0.0, 1.0) * size.width;
    final top = lane * Timeline.lane + 2;
    final end = moment.end;
    if (end == null || end <= moment.start) {
      return Rect.fromCenter(
        center: Offset(x(moment.start), top + 2.5),
        width: 6,
        height: 6,
      );
    }
    return Rect.fromLTRB(x(moment.start), top, x(end), top + 5);
  }
}

class _TimelinePainter extends CustomPainter {
  _TimelinePainter({
    required this.state,
    required this.placed,
    required this.selection,
    required this.focused,
    required this.track,
    required this.played,
    required this.head,
    required this.signal,
    required this.colorOf,
  });

  final PlaybackState state;
  final List<_Placed> placed;
  final Moment? selection;
  final String? focused;
  final Color track;
  final Color played;
  final Color head;
  final Color signal;
  final Color Function(String agentId) colorOf;

  @override
  void paint(Canvas canvas, Size size) {
    final duration = state.duration;
    final trackTop = size.height - 6 - Timeline.track;
    final line = Rect.fromLTWH(0, trackTop, size.width, Timeline.track);
    canvas.drawRRect(
      RRect.fromRectAndRadius(line, const Radius.circular(2)),
      Paint()..color = track,
    );
    if (duration <= 0) return;
    double x(double seconds) =>
        (seconds / duration).clamp(0.0, 1.0) * size.width;
    canvas.drawRRect(
      RRect.fromRectAndRadius(
        Rect.fromLTWH(0, trackTop, x(state.position), Timeline.track),
        const Radius.circular(2),
      ),
      Paint()..color = played,
    );
    for (final mark in placed) {
      final strong = mark.threadId == focused;
      final color = colorOf(mark.agentId).withValues(alpha: strong ? 1 : 0.7);
      final rect = mark.rect(size, duration);
      if (mark.moment.end == null || mark.moment.end! <= mark.moment.start) {
        canvas.drawCircle(rect.center, strong ? 4 : 3, Paint()..color = color);
      } else {
        canvas.drawRRect(
          RRect.fromRectAndRadius(
            rect.width < 3 ? rect.inflate(1) : rect,
            const Radius.circular(2.5),
          ),
          Paint()..color = color,
        );
      }
    }
    final selection = this.selection;
    if (selection != null) {
      final end = selection.end;
      final paint = Paint()..color = signal.withValues(alpha: 0.35);
      if (end == null || end <= selection.start) {
        canvas.drawRect(
          Rect.fromLTWH(x(selection.start) - 1, 0, 2, size.height),
          Paint()..color = signal,
        );
      } else {
        canvas.drawRect(
          Rect.fromLTRB(x(selection.start), 0, x(end), size.height),
          paint,
        );
      }
    }
    // The playhead.
    canvas.drawCircle(
      Offset(x(state.position), trackTop + Timeline.track / 2),
      5,
      Paint()..color = head,
    );
  }

  @override
  bool shouldRepaint(_TimelinePainter old) => true;
}

/// The parts of the frame the comments on this moment are on, outlined in
/// their agents' colors, and the person's mark and drag in signal.
class _FrameRegions extends CustomPainter {
  _FrameRegions({
    required this.comments,
    required this.at,
    required this.drag,
    required this.signal,
    required this.colorOf,
  }) : super(repaint: Listenable.merge([comments, comments.selection]));

  final CommentsController comments;
  final double at;
  final Rect? drag;
  final Color signal;
  final Color Function(String agentId) colorOf;

  @override
  void paint(Canvas canvas, Size size) {
    void box(Rect region, Color color, {required bool strong}) {
      final rect = Rect.fromLTWH(
        region.left * size.width,
        region.top * size.height,
        region.width * size.width,
        region.height * size.height,
      );
      canvas
        ..drawRect(
          rect,
          Paint()..color = color.withValues(alpha: strong ? 0.20 : 0.10),
        )
        ..drawRect(
          rect,
          Paint()
            ..color = color
            ..style = PaintingStyle.stroke
            ..strokeWidth = strong ? 2.5 : 1.5,
        );
    }

    for (final thread in comments.shown) {
      final moment = comments.moments[thread.id];
      final region = moment?.region;
      if (moment == null || region == null) continue;
      final strong = comments.focused == thread.id;
      if (!strong && !moment.covers(at)) continue;
      box(region, colorOf(thread.agentId), strong: strong);
    }
    final selected = comments.selection.value;
    if (selected is Moment && selected.region != null) {
      box(selected.region!, signal, strong: false);
    }
    final drag = this.drag;
    if (drag != null) box(drag, signal, strong: false);
  }

  @override
  bool shouldRepaint(_FrameRegions old) =>
      old.at != at || old.drag != drag || old.signal != signal;
}
