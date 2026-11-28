import 'dart:async';
import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../annotations.dart';
import '../backend.dart';
import '../controls.dart';
import '../diagnostics_record.dart' show ViewedFrom;
import '../identity.dart';
import '../mobile/metrics.dart';
import '../mobile/viewer_chrome.dart';
import '../pane_chrome.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../resources.dart' show formatBytes;
import '../review_answer.dart' show ReviewAnswering;
import '../scope.dart';
import '../selection.dart';
import '../paced.dart' show FilePreviews, PreviewBox, lostWhileReading;
import 'file_cache.dart';
import 'loading_progress.dart';
import 'preview_badge.dart';
import 'previews.dart';
import 'registry.dart';

/// How much of a video one read brings (the backend's cap).
const videoChunk = 8 * 1024 * 1024;

/// How much each read after the original's start brings: it comes in
/// pieces sized to the link, but a read given up on (the connection gone
/// over a minute) keeps none of it, so each is written as it comes and a
/// drop costs at most this (a preview carried on from 0).
const videoPiece = 1024 * 1024;

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
  const VideoViewer({super.key, required this.path, this.sealed});

  final String path;

  /// The seal whose kept version shows, rather than the file.
  final FileSeal? sealed;

  @override
  State<VideoViewer> createState() => _VideoViewerState();
}

class _VideoViewerState extends State<VideoViewer>
    with FileLoading<VideoViewer> {
  @override
  String get path => widget.path;

  @override
  FileSeal? get sealed => widget.sealed;

  @override
  Widget sealedViewer(FileSeal sealed) =>
      VideoViewer(path: path, sealed: sealed);

  @override
  String get loadingLabel => 'Loading video…';

  VideoPlayback? _playback;

  /// The copy on this device being played, deleted when the pane closes
  /// unless it's one this device keeps…
  File? _copy;

  /// …in this, which keeps it until let go.
  FileCache? _copyKeptIn;

  /// The copy of the version that stopped part way, carried on from when
  /// it's read again, and whether it's the preview's;
  /// deleted with the pane.
  ({String sha256, File copy, bool preview})? _partial;

  /// The version [_copy] holds, and whether it's the preview's: what a
  /// show of the same file plays on with.
  ({String sha256, bool preview})? _playing;

  /// A preview was due, but this Claktory can't make one.
  var _noPreview = false;

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

  /// The viewer's keys (Space, Escape): a click on the frame, the
  /// timeline or the player's buttons takes them.
  final _focus = FocusNode(debugLabel: 'video');

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final comments = this.comments;
    if (comments != _wired) {
      _wired?.removeListener(_commentsChanged);
      _wired = comments?..addListener(_commentsChanged);
      comments?.revealMoment = _play;
      comments?.rectOfMoment = _rectOfMoment;
    }
  }

  @override
  void dispose() {
    _wired?.removeListener(_commentsChanged);
    _close();
    _focus.dispose();
    super.dispose();
  }

  /// The part-way copy is of no use any more: gone.
  void _dropPartial() {
    final partial = _partial;
    _partial = null;
    if (partial != null && partial.copy.path != _copy?.path) {
      unawaited(partial.copy.delete().catchError((_) => partial.copy));
    }
  }

  void _close() {
    _dropPartial();
    unawaited(_playback?.dispose());
    _playback = null;
    final copy = _copy;
    final keptIn = _copyKeptIn;
    _copy = null;
    _copyKeptIn = null;
    _playing = null;
    if (copy != null && keptIn != null) {
      keptIn.release(copy);
    } else if (copy != null) {
      unawaited(copy.delete().catchError((_) => copy));
    }
  }

  /// The pointer is over the frame (for its Comment button).
  var _hovered = false;

  /// No pointer to hover with: the Comment button always shows.
  bool get _touch =>
      defaultTargetPlatform == TargetPlatform.iOS ||
      defaultTargetPlatform == TargetPlatform.android;

  /// The timeline's box, for where a moment is on screen.
  final _timeline = GlobalKey();

  /// [moment] on the timeline (global): its stretch, or a point.
  Rect? _rectOfMoment(Moment moment) {
    final box = _timeline.currentContext?.findRenderObject();
    final duration = _state.duration;
    if (box is! RenderBox || !box.attached || duration <= 0) return null;
    double x(double seconds) =>
        (seconds / duration).clamp(0.0, 1.0) * box.size.width;
    final end = moment.end;
    final local = Rect.fromLTRB(
      x(moment.start),
      0,
      end == null || end <= moment.start ? x(moment.start) : x(end),
      box.size.height,
    );
    return local.shift(box.localToGlobal(Offset.zero));
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
    // A player that failed goes, so the read shows how far it's got rather
    // than the failure until it's all here again.
    if (_playback?.state.value.error != null) _close();
    // Shown again (an agent's show, or Reload) while it plays: when the file
    // is still the version playing, it plays on from its copy, and only a
    // changed file is read again.
    final playing = _playing;
    if (playing != null &&
        _playback != null &&
        _copy != null &&
        !(playing.preview && wantOriginal)) {
      try {
        final now = await backend.readFile(
          path,
          maxBytes: 1,
          sealedSha256: sealedSha256,
        );
        if (!mounted || generation != _generation) return;
        if (now.error.isEmpty && now.sha256 == playing.sha256) {
          setState(() {
            _problem = null;
            failure = null;
            showingUnchecked = false;
          });
          return;
        }
      } on RequestFailure catch (error) {
        if (mounted && generation == _generation) {
          setState(() => failure = error.message);
        }
        return;
      }
    }
    // Kept on this device and still the file: played from there,
    // the original if it's here, else the preview made for this screen.
    final cache = keptCopies(backend);
    askingFor(backend);
    if (cache != null) {
      final found = await cache.lookup(backend, path, [
        originalVariant,
        if (!wantOriginal) previewVariant(previewScreen()),
      ]);
      if (!mounted || generation != _generation) return;
      if (found != null && found.kept.file.existsSync()) {
        _dropPartial();
        _size = found.kept.content.size.toInt();
        _noPreview = false;
        final content = found.kept.content;
        setState(
          () => seals =
              found.seals ?? (content.hasSeals() ? content.seals : null),
        );
        return _start(
          found.kept.file,
          found.kept.content,
          preview: found.kept.content.isPreview,
          keptIn: cache,
          unchecked: !found.current,
          from: ViewedFrom.kept,
        );
      }
    }
    setState(() {
      _problem = null;
      // Carrying on from a part read before: how far it got stays shown.
      if (_partial == null) _received = 0;
    });
    try {
      // Progress as each piece comes, not each range: its size
      // only once the first piece says it.
      void progress(int through, int size) {
        if (!mounted || generation != _generation) return;
        setState(() {
          _received = through;
          _size = size;
          // In the badge, while the preview plays.
          if (originalLoad is OriginalLoading) {
            originalLoad = OriginalLoading(through, size);
          }
        });
      }

      // Stopped part way before (the connection didn't come back in
      // time): from there, while the file is still that version.
      final partial = _partial;
      var resumeAt = 0;
      FileContent? head;
      // A preview made for this screen, on a link too slow for the
      // original; the original once it's asked for.
      PreviewBox? preview;
      // (A preview's, unless it's the original that's wanted now.)
      if (partial != null &&
          !(partial.preview && wantOriginal) &&
          await partial.copy.exists()) {
        if (partial.preview) preview = previewScreen();
        head = await backend.readFile(
          path,
          sealedSha256: sealedSha256,
          maxBytes: 1,
          expectedSha256: partial.sha256,
          preview: preview,
        );
        if (!mounted || generation != _generation) return;
        if (head.error.isEmpty) {
          resumeAt = await partial.copy.length();
        } else {
          head = null;
        }
      }
      if (head == null) {
        _dropPartial();
        preview = null;
      }
      // The original's start, stopping once it's clear the rest would be
      // slow to come: a preview comes instead, from the start.
      var first =
          head ??
          await backend.readFile(
            path,
            sealedSha256: sealedSha256,
            maxBytes: videoChunk,
            onProgress: progress,
            carryOn: wantOriginal ? null : stopWhenSlow(backend),
          );
      if (head == null &&
          !wantOriginal &&
          first.error.isEmpty &&
          first.size.toInt() >= previewFrom &&
          // Stopped short: a whole chunk that came isn't thrown away.
          first.data.length < math.min(videoChunk, first.size.toInt()) &&
          slowFor(backend, first.size.toInt() - first.data.length)) {
        if (!mounted || generation != _generation) return;
        preview = previewScreen();
        first = await backend.readFile(
          path,
          sealedSha256: sealedSha256,
          maxBytes: videoPiece,
          onProgress: progress,
          preview: preview,
        );
      }
      if (!mounted || generation != _generation) return;
      if (first.error.isNotEmpty) {
        if (_replacing) return _originalFailed();
        setState(() => file = first);
        return;
      }
      // Asked for and not made: the original came, and the rest of it is
      // read as the original.
      if (!first.isPreview) preview = null;
      _size = first.size.toInt();
      // What a change on the machine is compared with, and why a lost
      // connection doesn't start it again: the read carries on by itself.
      // The preview playing meanwhile keeps its own.
      setState(() {
        _noPreview = first.preview == FilePreview.FILE_PREVIEW_UNAVAILABLE;
        if (!_replacing) file = first.deepCopy()..data = const [];
        seals = first.hasSeals() ? first.seals : null;
        failure = null;
      });
      final name = path.split('/').last;
      final version = first.sha256.isEmpty
          ? '${DateTime.now().microsecondsSinceEpoch}'
          : first.sha256.substring(0, first.sha256.length.clamp(0, 16));
      var copy = head != null
          ? partial!.copy
          : File(
              '${Directory.systemTemp.path}/claktory-$version'
              '${preview != null ? '-preview' : ''}-$name',
            );
      // Never the copy the player has open: written over and then deleted
      // with the player it replaces, the new player had nothing to open.
      if (copy.path == _copy?.path) {
        copy = File('${copy.parent.path}/again-${copy.uri.pathSegments.last}');
      }
      if (first.sha256.isNotEmpty) {
        _partial = (sha256: first.sha256, copy: copy, preview: preview != null);
      }
      final sink = copy.openWrite(
        mode: head != null ? FileMode.append : FileMode.write,
      );
      try {
        var offset = resumeAt;
        if (head == null) {
          sink.add(first.data);
          offset = first.data.length;
        }
        setState(() => _received = offset);
        while (offset < _size) {
          final chunk = await backend.readFile(
            path,
            sealedSha256: sealedSha256,
            maxBytes: videoPiece,
            offset: offset,
            expectedSha256: first.sha256,
            onProgress: progress,
            preview: preview,
          );
          if (!mounted || generation != _generation) return;
          if (chunk.error == 'changed') {
            // Changed while it came: start again with the new one.
            _dropPartial();
            unawaited(reload());
            return;
          }
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
            _dropPartial();
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
      _partial = null;
      // As the first piece said it, a preview or not.
      final whole = first.deepCopy()
        ..data = const []
        ..clearOffset()
        ..clearTruncated();
      // Kept on this device for next time, unless it's more than
      // this Claktory keeps: then it's this pane's, as before.
      final kept = cache == null
          ? null
          : await cache.adopt(
              path,
              preview != null ? previewVariant(preview) : originalVariant,
              whole,
              copy,
            );
      if (kept != null) cache!.hold(kept.file);
      if (!mounted || generation != _generation) {
        if (kept != null) cache!.release(kept.file);
        return;
      }
      await _start(
        kept?.file ?? copy,
        whole,
        preview: preview != null,
        keptIn: kept == null ? null : cache,
        from: ViewedFrom.read,
        bytes: _size - resumeAt,
      );
      if (kept != null) cache!.release(kept.file);
    } on RequestFailure catch (error) {
      if (_replacing) return _originalFailed(error);
      if (mounted) setState(() => failure = error.message);
    } on FileSystemException catch (error) {
      _fail("Couldn't load the video: ${error.message}");
    }
  }

  /// Plays [copy], [first]'s version (a preview or not), in place of
  /// what played; a copy this device keeps ([keptIn]) is held while it
  /// plays, and [unchecked] when it couldn't be checked against the file.
  Future<void> _start(
    File copy,
    FileContent first, {
    required bool preview,
    FileCache? keptIn,
    bool unchecked = false,
    required ViewedFrom from,
    int bytes = 0,
  }) async {
    keptIn?.hold(copy);
    final playback = newVideoPlayback();
    // Where it was, when it's the same version (the original in place
    // of its preview); a new version starts at its beginning.
    final was = _playing?.sha256 == first.sha256
        ? _playback?.state.value.position
        : null;
    _close();
    _copy = copy;
    _copyKeptIn = keptIn;
    _playing = first.sha256.isEmpty
        ? null
        : (sha256: first.sha256, preview: preview);
    _playback = playback;
    setState(() {
      file = first.deepCopy()
        ..data = const []
        ..clearOffset()
        ..clearTruncated();
      failure = null;
      _problem = null;
      showingUnchecked = unchecked;
      originalLoad = const OriginalNotLoaded();
    });
    playback.state.addListener(_playbackChanged);
    await playback.open(copy.path);
    if (was != null && was > 0) await playback.seek(was);
    if (!mounted) return;
    noteShown(first, from: from, bytes: bytes);
    final comments = this.comments;
    if (comments != null && first.sha256.isNotEmpty) {
      comments.show(kind: 'video', text: '', version: first.sha256);
    }
  }

  void _fail(String problem) {
    if (_replacing) return _originalFailed();
    if (mounted) setState(() => _problem = problem);
  }

  /// The original is coming in place of the preview playing.
  bool get _replacing =>
      originalLoad is OriginalLoading && _playback != null && mounted;

  /// It couldn't: the preview plays on, and the badge says so; lost with
  /// the connection, it carries on once that's back ([originalFailed]).
  void _originalFailed([RequestFailure? error]) => originalFailed(error);

  /// The original in place of the preview, read as a video is: the
  /// preview plays meanwhile, and the original carries on from where it
  /// was.
  @override
  Future<void> loadOriginal() async {
    if (originalLoad is OriginalLoading) return;
    wantOriginal = true;
    setState(() => originalLoad = const OriginalLoading(0, 0));
    await reload();
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

  /// [moment] as the bar says it: 0:12, or 0:12–0:20.
  static String _when(Moment moment) {
    final end = moment.end;
    return end == null
        ? formatTime(moment.start)
        : '${formatTime(moment.start)}–${formatTime(end)}';
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

  /// The video's size for its header: the original's, also while its
  /// preview plays, as an image's; none while a preview
  /// plays from a Claktory that doesn't say the original's, rather than
  /// the preview's as if it were the original's.
  static Size? _shownSize(FileContent? file, PlaybackState state) {
    if (file != null && file.isPreview) {
      if (file.originalPixelWidth <= 0 || file.originalPixelHeight <= 0) {
        return null;
      }
      return Size(
        file.originalPixelWidth.toDouble(),
        file.originalPixelHeight.toDouble(),
      );
    }
    return state.ready && state.size != Size.zero ? state.size : null;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final file = this.file;
    final state = _state;
    final phone = isPhoneLayout(context);
    // Answering a review, nothing but the answer: no comments meanwhile.
    final comments =
        file != null &&
            file.sha256.isNotEmpty &&
            state.ready &&
            !ReviewAnswering.of(context)
        ? this.comments
        : null;
    final details = this.details(
      [
        if (_shownSize(file, state) case final size?)
          '${size.width.round()} × ${size.height.round()}',
        if (state.ready) formatTime(state.duration),
        if (file != null && file.error.isEmpty)
          formatBytes(file.isPreview ? file.originalSize : file.size),
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
        // The read failed (the connection didn't come back in time): it
        // says so, and Try again reads it again.
        // Lost with the connection, it's read again once that's back
        // (FileLoading): till then it waits, rather than saying it failed.
        // Given up on after the drop, it says so offline too; lost a
        // moment before the connection says it's down, it waits all the
        // same.
        (failure != null &&
                (failure == lostWhileReading ||
                    AppScope.of(context).connected.value &&
                        !lostWithConnection(failure))
            ? _Failed(text: failure!, path: failurePath, onRetry: reload)
            : null);
    final Widget body;
    if (problem != null) {
      body = problem;
    } else if (!state.ready) {
      body = LoadingProgress(
        name: path.split('/').last,
        label: 'Loading video…',
        received: _received,
        size: _size,
        connected: AppScope.of(context).connected,
        note: _noPreview
            ? "Full size: this Claktory can't make a smaller copy."
            : null,
      );
    } else if (phone) {
      // Edge to edge, its controls over its foot (mobile/viewer_chrome):
      // they fade while it plays, and stay while a comment is placed.
      body = HoldViewerChrome(
        when: _marking,
        child: FadeViewerChrome(
          when: state.playing && !_marking,
          child: Stack(
            children: [
              Positioned.fill(child: _frame(palette, state, comments)),
              if (!_marking)
                Center(
                  child: _CenterPlay(
                    playing: state.playing,
                    onTap: _togglePlaying,
                  ),
                ),
            ],
          ),
        ),
      );
    } else {
      body = Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(
            child: Stack(
              children: [
                Positioned.fill(child: _frame(palette, state, comments)),
                // A preview says so, clear of Comment at the top.
                if (previewBadge() case final badge?)
                  Positioned(
                    left: Space.m,
                    right: Space.m,
                    bottom: Space.m,
                    child: Align(alignment: Alignment.bottomLeft, child: badge),
                  ),
                // Comment at the pane's corner, not the frame's: a tall
                // video in a short pane is a sliver its words can't fit in
                // . A phone has it in the bar under the video.
                if (comments != null)
                  Positioned(
                    top: Space.m,
                    right: Space.m,
                    left: Space.m,
                    child: Align(
                      alignment: Alignment.topRight,
                      child: CommentPick(
                        picking: _marking,
                        visible: _hovered || _touch,
                        hint:
                            '${_touch ? 'Tap' : 'Click'} the timeline for a '
                            'moment, drag along it for a stretch, or '
                            '${_touch ? 'tap' : 'click'} the frame for a spot.',
                        onToggle: _toggleMarking,
                      ),
                    ),
                  ),
                // Whether the pointer is over the video, the button
                // included, for Comment; it takes no clicks.
                if (comments != null)
                  Positioned.fill(
                    child: MouseRegion(
                      opaque: false,
                      hitTestBehavior: HitTestBehavior.translucent,
                      onEnter: (_) => setState(() => _hovered = true),
                      onExit: (_) => setState(() => _hovered = false),
                    ),
                  ),
              ],
            ),
          ),
          _Controls(
            timelineKey: _timeline,
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
    // A click on the player itself takes the keys; one in a comment's card
    // leaves them to its field.
    final player = Listener(
      onPointerDown: (_) => _focus.requestFocus(),
      child: body,
    );
    final ready = problem == null && state.ready;
    return PaneScaffold(
      share: sharePath,
      details: details,
      controls: controls,
      comments: comments == null ? null : CommentsButton(controller: comments),
      actions: [PaneAction('Reload', reload), ...sealActions()],
      notice: sealNotice(),
      phoneKind: PhoneViewerKind.video,
      // One row over its foot, as the system's player has it; a preview
      // says so above it.
      phoneOverlay: !phone || !ready
          ? null
          : Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (previewBadge() case final badge?)
                  Padding(
                    padding: const EdgeInsets.fromLTRB(
                      Space.m,
                      Space.s,
                      Space.m,
                      0,
                    ),
                    child: Align(alignment: Alignment.centerLeft, child: badge),
                  ),
                _Controls(
                  timelineKey: _timeline,
                  state: state,
                  comments: comments,
                  marking: _marking,
                  onPlay: _togglePlaying,
                  onSeek: (seconds) => _playback?.seek(seconds),
                  onMark: _markTime,
                  onComment: comments != null && !_marking
                      ? _toggleMarking
                      : null,
                ),
              ],
            ),
      // Placing a comment: how, Cancel and Comment, under the controls.
      toolbar: comments == null || (phone && !_marking)
          ? null
          : ValueListenableBuilder(
              valueListenable: comments.selection,
              builder: (context, picked, _) => CommentModeBar(
                marking: _marking,
                onToggle: _toggleMarking,
                // The moment it was paused at to begin with: Comment
                // twice writes about what's on screen.
                hint: picked is Moment
                    ? 'At ${_when(picked)}. Tap the timeline for another '
                          'moment, or drag across the frame.'
                    : 'Tap the timeline, or drag across the frame.',
                action: 'Comment',
                actionPrimary: true,
                onAction: picked is Moment ? comments.comment : null,
              ),
            ),
      // Not while a comment or an answer is being typed.
      body: ViewerKeys(
        bindings: {
          const SingleActivator(LogicalKeyboardKey.space): _togglePlaying,
          const SingleActivator(LogicalKeyboardKey.escape): () {
            if (_marking) _toggleMarking();
          },
        },
        child: Focus(
          focusNode: _focus,
          child: ClipRect(
            child: ColoredBox(
              // On a phone, the page's black shows round it.
              color: phone ? const Color(0x00000000) : palette.canvas,
              child: comments == null
                  ? player
                  : Commented(controller: comments, child: player),
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
    // On a phone, edge to edge, and a tap shows or hides its controls (as
    // the system's player): Play is among them, and in the middle.
    final phone = isPhoneLayout(context);
    return Padding(
      padding: phone ? EdgeInsets.zero : const EdgeInsets.all(Space.l),
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
                    onTap: _marking || phone
                        ? null
                        : () {
                            comments?.letGo();
                            _togglePlaying();
                          },
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
                          ring: palette.canvas,
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
                        onTapUp: (details) {
                          final at = fraction(details.localPosition);
                          _markRegion(Rect.fromLTWH(at.dx, at.dy, 0, 0));
                        },
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
                          if (from == null || to == null) return;
                          final area = Region(Rect.fromPoints(from, to));
                          // Too small to be an area: the spot it began on.
                          _markRegion(
                            area.isEmpty
                                ? Rect.fromLTWH(from.dx, from.dy, 0, 0)
                                : area.rect,
                          );
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

/// Play or Pause in the middle of a phone's video, with its controls:
/// they fade together (mobile/viewer_chrome).
class _CenterPlay extends StatelessWidget {
  const _CenterPlay({required this.playing, required this.onTap});

  final bool playing;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final shown = ViewerChrome.maybeOf(context)?.shown ?? true;
    return IgnorePointer(
      ignoring: !shown,
      child: ExcludeSemantics(
        // Play is in the row of controls too: once is enough to hear.
        child: AnimatedOpacity(
          duration: Motion.base,
          opacity: shown ? 1 : 0,
          child: Pressable(
            onTap: onTap,
            builder: (context, pressed) => Container(
              width: 64,
              height: 64,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: palette.sidebar.withValues(alpha: pressed ? 0.7 : 0.92),
              ),
              alignment: Alignment.center,
              child: ExtensionGlyph(
                icon: playing ? 'pause' : 'play',
                name: playing ? 'Pause' : 'Play',
                color: palette.text,
                size: 28,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The video couldn't be read: why, and Try again.
class _Failed extends StatelessWidget {
  const _Failed({
    required this.text,
    required this.path,
    required this.onRetry,
  });

  final String text;
  final String path;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) => Center(
    child: SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          ViewerProblem(text, path: path),
          Button(label: 'Try again', onPressed: onRetry),
        ],
      ),
    ),
  );
}

/// Under the frame, behind a hairline: play or pause, the time, and the
/// timeline.
class _Controls extends StatelessWidget {
  const _Controls({
    required this.timelineKey,
    required this.state,
    required this.comments,
    required this.marking,
    required this.onPlay,
    required this.onSeek,
    required this.onMark,
    this.onComment,
  });

  /// On a phone, Comment at the row's end: picking what to comment on.
  final VoidCallback? onComment;

  final GlobalKey timelineKey;
  final PlaybackState state;
  final CommentsController? comments;
  final bool marking;
  final VoidCallback onPlay;
  final void Function(double seconds) onSeek;
  final void Function(double seconds, {double? until}) onMark;

  Widget _phone(BuildContext context) {
    final palette = Palette.of(context);
    final time = TypeScale.of(context).caption.copyWith(
      color: palette.textMuted,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final onComment = this.onComment;
    // One row over the video's foot, as the system's player has it: Play,
    // where it is, the timeline, how long, and Comment. Its words grow
    // only so far (as the Preview badge's), so the timeline keeps its
    // room on a small iPhone at the largest text.
    final play = GlyphButton(
      label: state.playing ? 'Pause' : 'Play',
      active: false,
      onTap: onPlay,
      glyph: (color) => ExtensionGlyph(
        icon: state.playing ? 'pause' : 'play',
        name: state.playing ? 'Pause' : 'Play',
        color: color,
      ),
    );
    final timeline = Timeline(
      key: timelineKey,
      state: state,
      comments: comments,
      marking: marking,
      onSeek: onSeek,
      onMark: onMark,
    );
    final position = ExcludeSemantics(
      child: Text(formatTime(state.position), style: time),
    );
    final duration = ExcludeSemantics(
      child: Text(formatTime(state.duration), style: time),
    );
    final comment = onComment == null
        ? null
        : Button(label: 'Comment', onPressed: onComment);
    // One row over the video's foot: Play, where it is, the timeline, how
    // long, and Comment. Its words grow only so far (as the Preview
    // badge's); where even so the timeline would get less than a thumb's
    // room (a small iPhone at the largest text), the times go under it,
    // as the system's player has them.
    return MediaQuery.withClampedTextScaling(
      maxScaleFactor: maxPreviewBadgeTextScale,
      child: Builder(
        builder: (context) {
          final scaler = MediaQuery.textScalerOf(context);
          double width(String text, TextStyle style) {
            final painter = TextPainter(
              text: TextSpan(text: text, style: style),
              textScaler: scaler,
              textDirection: TextDirection.ltr,
              maxLines: 1,
            )..layout();
            final width = painter.width;
            painter.dispose();
            return width;
          }

          return LayoutBuilder(
            builder: (context, constraints) {
              final times =
                  width(formatTime(state.position), time) +
                  width(formatTime(state.duration), time) +
                  2 * Space.s;
              final rest =
                  Metrics.of(context).target +
                  2 * Space.xs +
                  Space.s +
                  (comment == null
                      ? 0
                      : Space.xs +
                            width('Comment', PhoneType.label) +
                            2 * Touch.gutter);
              final tight = constraints.maxWidth - rest - times < 120;
              if (tight) {
                // The timeline across the whole width (clear of the edge
                // a swipe back starts from), the times under its ends,
                // and Play and Comment in a row below.
                return Unselectable(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Padding(
                        padding: const EdgeInsets.symmetric(
                          horizontal: Space.xl,
                        ),
                        child: timeline,
                      ),
                      Padding(
                        padding: const EdgeInsets.symmetric(
                          horizontal: Space.xl,
                        ),
                        child: Row(
                          mainAxisAlignment: MainAxisAlignment.spaceBetween,
                          children: [position, duration],
                        ),
                      ),
                      Padding(
                        padding: const EdgeInsets.symmetric(
                          horizontal: Space.xs,
                        ),
                        child: Row(
                          children: [
                            play,
                            const Spacer(),
                            ?comment,
                            const SizedBox(width: Space.s),
                          ],
                        ),
                      ),
                    ],
                  ),
                );
              }
              return Unselectable(
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: Space.xs),
                  child: Row(
                    children: [
                      play,
                      position,
                      const SizedBox(width: Space.s),
                      Expanded(child: timeline),
                      const SizedBox(width: Space.s),
                      duration,
                      if (comment != null) ...[
                        const SizedBox(width: Space.xs),
                        comment,
                      ],
                      const SizedBox(width: Space.s),
                    ],
                  ),
                ),
              );
            },
          );
        },
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    if (isPhoneLayout(context)) return _phone(context);
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
                key: timelineKey,
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

  /// The comment whose mark is under [local], if any: within a
  /// fingertip's reach on a phone.
  String? _hit(Offset local, Size size, List<_Placed> placed, {double by = 4}) {
    for (final mark in placed.reversed) {
      if (mark.rect(size, widget.state.duration).inflate(by).contains(local)) {
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
        final phone = isPhoneLayout(context);
        // A finger's target on a phone: the track in the middle of 44pt.
        final pad = phone && height < Touch.target
            ? (Touch.target - height) / 2
            : 0.0;
        return LayoutBuilder(
          builder: (context, constraints) {
            final size = Size(constraints.maxWidth, height);
            final timeline = MouseRegion(
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
                  final hit = _hit(
                    details.localPosition - Offset(0, pad),
                    size,
                    placed,
                    by: phone ? 10 : 4,
                  );
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
                child: Padding(
                  padding: EdgeInsets.symmetric(vertical: pad),
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
              ),
            );
            return phone ? _adjustable(comments, timeline) : timeline;
          },
        );
      },
    );
  }

  /// On a phone, for VoiceOver: an adjustable slider, a swipe up or down
  /// moving the time by a twentieth of the video (between 1 and 10
  /// seconds); while marking, it moves the moment being marked.
  Widget _adjustable(CommentsController? comments, Widget timeline) {
    final state = widget.state;
    final duration = state.duration;
    final picked = comments?.selection.value;
    final at = widget.marking && picked is Moment
        ? picked.start
        : state.position;
    final step = (duration / 20).clamp(1.0, 10.0);
    double by(double delta) => (at + delta).clamp(0.0, duration);
    void move(double delta) {
      if (widget.marking) {
        widget.onMark(by(delta));
      } else {
        widget.onSeek(by(delta));
      }
    }

    String said(double seconds) =>
        '${formatTime(seconds)} of ${formatTime(duration)}';
    return Semantics(
      container: true,
      slider: true,
      label: widget.marking ? 'Moment to comment on' : 'Position',
      value: said(at),
      increasedValue: said(by(step)),
      decreasedValue: said(by(-step)),
      onIncrease: duration <= 0 ? null : () => move(step),
      onDecrease: duration <= 0 ? null : () => move(-step),
      child: ExcludeSemantics(child: timeline),
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
    required this.ring,
    required this.colorOf,
  }) : super(repaint: Listenable.merge([comments, comments.selection]));

  final CommentsController comments;
  final double at;
  final Rect? drag;
  final Color signal;

  /// Around each pin, so it shows on any frame: the canvas.
  final Color ring;
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
      if (rect.isEmpty) {
        // A spot: only its pin.
        if (rect.width == 0 && rect.height == 0) {
          paintPin(canvas, rect.topLeft, color: color, ring: ring);
        }
        return;
      }
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

    final pins = comments.pins;
    final placed = <(Offset, Color, int, bool)>[];
    for (final thread in comments.shown) {
      final moment = comments.moments[thread.id];
      final region = moment?.region;
      if (moment == null || region == null) continue;
      final strong = comments.focused == thread.id;
      if (!strong && !moment.covers(at)) continue;
      final color = colorOf(thread.agentId);
      if (region.width > 0 || region.height > 0) {
        box(region, color, strong: strong);
      }
      final number = pins[thread.id];
      if (number != null) {
        final pin = Offset(region.left * size.width, region.top * size.height);
        placed.add((pin, color, number, strong));
      }
    }
    placed.sort((a, b) => (a.$4 ? 1 : 0) - (b.$4 ? 1 : 0));
    for (final (pin, color, number, strong) in placed) {
      paintPin(
        canvas,
        pin,
        color: color,
        ring: ring,
        number: number,
        strong: strong,
      );
    }
    final selected = comments.composing ?? comments.selection.value;
    if (selected is Moment && selected.region != null) {
      box(selected.region!, signal, strong: false);
    }
    final drag = this.drag;
    if (drag != null) box(drag, signal, strong: false);
  }

  @override
  bool shouldRepaint(_FrameRegions old) =>
      old.at != at ||
      old.drag != drag ||
      old.signal != signal ||
      old.ring != ring;
}
