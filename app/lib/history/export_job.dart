// History's video, made in the background (#458, Matt's feedback through
// Bruno 44256 and 44277): exporting takes a while, so once the person asks
// for it, it carries on whatever they do meanwhile (another agent, the Map
// closed, another window) until it's done or they cancel it. Its progress,
// then where the video is, shows along the top of the window, wherever
// they are ([ReplayJobBanner]).
//
// Each frame is drawn in a render tree of its own, off the screen, so
// nothing on screen has to stay open for it and no frame waits for the
// display. A finished video is saved at once, before anything else, where
// everyone knows to look: on a Mac in ~/Movies/Claktory, with a dated name;
// on an iPhone or iPad in Photos (or, when Photos isn't allowed, to Files
// with the person's say). It's never left in a temporary folder, and never
// deleted unless it's saved or the person says so.
//
// Nothing of it outlives the app (tenet 7): it's what this device is doing
// now, not state of the Claktory's.
import 'dart:async';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../mobile/drawer.dart' show largeText;
import '../mobile/metrics.dart' show Metrics, TypeScale;
import '../mobile/presentation.dart' show SheetAction, showActions;
import '../mobile/share.dart' show globalRectOf;
import '../protocol/factory/v1/factory.pb.dart' show Person;
import '../your_claktories.dart' show bannersNeedAbove;
import '../share_files.dart'
    show
        ShareUnavailable,
        keepMadeFile,
        markShared,
        releaseMadeFile,
        shareableCopy,
        shareFile,
        showInFinder;
import 'export.dart';

const _writer = MethodChannel('claktory/replay-writer');

/// Where a finished video goes on a Mac.
Directory get replayFolder =>
    debugReplayFolder ??
    Directory('${Platform.environment['HOME'] ?? ''}/Movies/Claktory');

/// A test's: makes only this many frames.
@visibleForTesting
int? debugReplayFrames;

/// A test's frame grab in place of rendering one ([RenderRepaintBoundary]
/// to the frame's RGBA bytes).
@visibleForTesting
Future<Uint8List> Function(RenderRepaintBoundary stage, ReplayShape shape)?
debugReplayCapture;

/// A test's own folder in place of ~/Movies/Claktory.
@visibleForTesting
Directory? debugReplayFolder;

/// How [replayFolder] is named to the person.
const replayFolderName = 'Movies › Claktory';

/// A finished video's name: when it was made, as the Mac names a
/// screenshot ("Claktory History 2026-11-26 at 17.07.12.mp4").
String replayFileName(DateTime at) {
  String two(int n) => n.toString().padLeft(2, '0');
  return 'Claktory History ${at.year}-${two(at.month)}-${two(at.day)} at '
      '${two(at.hour)}.${two(at.minute)}.${two(at.second)}.mp4';
}

/// Where a video was saved.
enum ReplaySavedTo {
  /// [replayFolder], on a Mac.
  movies,

  /// The Photos library, on an iPhone or iPad.
  photos,

  /// A place in Files the person chose, on an iPhone or iPad.
  files,
}

enum ReplayJobState {
  /// Drawing and encoding its frames.
  making,

  /// Made, and being saved.
  saving,

  /// Saved where [ReplayJob.savedTo] says.
  saved,

  /// Made, but not saved anywhere yet: Photos refused it.
  unsaved,

  /// It couldn't be made or saved: [ReplayJob.error] says why.
  failed,
}

/// One video being made, from start to where it's saved.
class ReplayJob extends ChangeNotifier {
  ReplayJob({
    required this.video,
    required this.shape,
    required this.privacy,
    required this.person,
    required this.media,
    this.style = const TextStyle(),
    DateTime Function()? clock,
  }) : _clock = clock ?? DateTime.now;

  /// Its own playhead over the history, from the first frame.
  final ReplayVideo video;
  final ReplayShape shape;
  final ReplayPrivacy privacy;
  final Person person;

  /// The window's, as it was when it was asked for: the frames look as the
  /// preview did, whatever happens to the window after.
  final MediaQueryData media;

  /// The window's text style, which the stage's words start from.
  final TextStyle style;
  final DateTime Function() _clock;

  var state = ReplayJobState.making;

  /// How much of it is made, 0…1.
  double progress = 0;

  /// Why it failed, or why it couldn't be saved or shared, in words to
  /// show; null while all's well.
  String? error;

  /// The video: where it was saved on a Mac, else where it's kept until
  /// it is (and for Share…).
  File? file;
  ReplaySavedTo? savedTo;

  var _cancelled = false;
  var _disposed = false;

  bool get done =>
      state == ReplayJobState.saved ||
      state == ReplayJobState.unsaved ||
      state == ReplayJobState.failed;

  /// Stops it while it's being made; what was made goes.
  void cancel() => _cancelled = true;

  void _changed() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _cancelled = true;
    made.dispose();
    video.dispose();
    super.dispose();
  }

  /// Makes it, then saves it. Completes once it's saved, failed or
  /// cancelled (then false).
  Future<bool> run() async {
    final words = ShareWords(video.scene, privacy);
    final frames = math.min(debugReplayFrames ?? video.frames, video.frames);
    final stage = _Offscreen(shape.logical);
    String? path;
    try {
      path = await _writer.invokeMethod<String>('start', {
        'width': shape.width,
        'height': shape.height,
        'fps': replayFps,
      });
      // However long it takes, no sweep of the shares takes it.
      if (path != null) await keepMadeFile(File(path));
      var shown = -1;
      for (var i = 0; i < frames; i++) {
        // Away from the app, an iPhone's encoder stops: wait to be back.
        await _foreground();
        if (_cancelled) throw const _Cancelled();
        video.step(i, words);
        final boundary = stage.draw(
          MediaQuery(
            data: media,
            child: Directionality(
              textDirection: TextDirection.ltr,
              child: DefaultTextStyle(
                style: style,
                child: ReplayStage(
                  video: video,
                  shape: shape,
                  privacy: privacy,
                  person: person,
                ),
              ),
            ),
          ),
        );
        final rgba = await captureReplayFrame(boundary, shape);
        if (_cancelled) throw const _Cancelled();
        await _writer.invokeMethod<void>('frame', {'rgba': rgba});
        await _screenFrame(i);
        progress = (i + 1) / frames;
        // A step a percent, not a rebuild a frame.
        final percent = (progress * 100).floor();
        if (percent != shown) {
          shown = percent;
          _changed();
        }
      }
      if (_cancelled) throw const _Cancelled();
      path = await _writer.invokeMethod<String>('finish');
      if (path == null) throw PlatformException(code: 'failed');
      if (!privacy.dates) await clearMp4Dates(File(path));
      state = ReplayJobState.saving;
      _changed();
      await _save(File(path));
      return true;
    } on _Cancelled {
      unawaited(_writer.invokeMethod<void>('cancel').catchError((_) {}));
      return false;
    } on Object catch (error) {
      unawaited(_writer.invokeMethod<void>('cancel').catchError((_) {}));
      if (_disposed) return false;
      state = ReplayJobState.failed;
      this.error = switch (error) {
        PlatformException(code: 'limit') => "it's too long.",
        FileSystemException(:final message, :final osError) =>
          osError?.errorCode == 28
              ? "there isn't enough space on this Mac. Free up some space, "
                    'then try again.'
              : '$message.',
        _ => null,
      };
      _changed();
      return false;
    } finally {
      stage.dispose();
    }
  }

  /// Changes each frame made: [ReplayJobBanner] draws a pixel from it, so
  /// the screen has something to draw (below).
  final made = ValueNotifier<int>(0);

  /// Whether the last frame on screen came in time; when it didn't, it's
  /// asked for only now and then.
  var _screenOn = true;

  /// A frame on screen every so many of the video's.
  static const _screenEvery = 8;

  /// A frame drawn on the screen between two of the video's. Impeller
  /// keeps what it draws a frame from in a buffer that only a frame on
  /// screen rewinds: frame after frame of a video with none between grew
  /// it by about 5 MB each (Polpo: 8.7 GB for a minute's video, the engine's
  /// snapshot path, flutter/flutter#192097). It's waited for only briefly:
  /// a hidden window draws nothing. Every [_screenEvery]th video frame
  /// only: the screen's frame redraws the whole window, which cost more
  /// than the video's own (Polpo 50054: 64% of the raster thread), and the
  /// buffer grows by only a few such frames meanwhile.
  Future<void> _screenFrame(int i) async {
    if (i % _screenEvery != 0) return;
    made.value = i;
    if (!_screenOn && i % (_screenEvery * 4) != 0) return;
    final binding = SchedulerBinding.instance;
    binding.scheduleFrame();
    final drawn = Completer<bool>();
    final timer = Timer(const Duration(milliseconds: 60), () {
      if (!drawn.isCompleted) drawn.complete(false);
    });
    unawaited(
      binding.endOfFrame.then((_) {
        if (!drawn.isCompleted) drawn.complete(true);
      }),
    );
    _screenOn = await drawn.future;
    timer.cancel();
  }

  /// Until the app is in front again, where its encoder needs it.
  Future<void> _foreground() async {
    if (defaultTargetPlatform != TargetPlatform.iOS) return;
    final binding = WidgetsBinding.instance;
    while (!_cancelled &&
        binding.lifecycleState != null &&
        binding.lifecycleState != AppLifecycleState.resumed) {
      await Future<void>.delayed(const Duration(milliseconds: 200));
    }
  }

  /// [made] where it belongs: on a Mac moved into [replayFolder]; on an
  /// iPhone or iPad added to Photos, and kept here (where nothing sweeps it)
  /// when Photos says no.
  Future<void> _save(File made) async {
    if (defaultTargetPlatform == TargetPlatform.macOS) {
      final folder = replayFolder;
      await folder.create(recursive: true);
      final to = _freeName(folder, replayFileName(_clock()));
      File saved;
      try {
        saved = await made.rename(to.path);
      } on FileSystemException {
        // On another volume: copied there instead.
        saved = await made.copy(to.path);
        await made.delete();
      }
      unawaited(_deleteFolder(made.parent));
      file = saved;
      savedTo = ReplaySavedTo.movies;
      state = ReplayJobState.saved;
      _changed();
      return;
    }
    file = made;
    try {
      await _writer.invokeMethod<void>('saveToPhotos', {'path': made.path});
      savedTo = ReplaySavedTo.photos;
      state = ReplayJobState.saved;
      await releaseMadeFile(made);
    } on PlatformException catch (failure) {
      // Not deleted until it's saved somewhere: kept from the sweep.
      await keepMadeFile(made);
      state = ReplayJobState.unsaved;
      error = failure.code == 'denied'
          ? "Claktory isn't allowed to add to your Photos. Save it to Files, "
                'or allow it for Claktory in the Settings app.'
          : "Couldn't add it to your Photos. Save it to Files instead.";
    }
    _changed();
  }

  /// Saves it to Files, where the person chooses (an iPhone or iPad).
  Future<void> saveToFiles() async {
    final made = file;
    if (made == null) return;
    try {
      final saved =
          await _writer.invokeMethod<bool>('saveToFiles', {
            'path': made.path,
            'name': replayFileName(_clock()),
          }) ??
          false;
      if (!saved) return;
      savedTo = ReplaySavedTo.files;
      state = ReplayJobState.saved;
      error = null;
      await releaseMadeFile(made);
    } on PlatformException catch (failure) {
      error = "Couldn't save it to Files: ${failure.message ?? failure.code}.";
    }
    _changed();
  }

  /// Opens the share sheet for it, pointing at [from]. Says why when the
  /// sheet can't open, rather than nothing (Matt's export, Bruno 44170).
  Future<void> share({Rect? from}) async {
    final made = file;
    if (made == null) return;
    // Its name in Movies says when it was made: without Dates, a copy
    // that doesn't goes instead (Tiziano).
    final copy = savedTo == ReplaySavedTo.movies && !privacy.dates;
    File? sent;
    try {
      sent = copy ? await shareableCopy(made, 'Claktory History.mp4') : made;
      final went = await shareFile(sent, mime: 'video/mp4', from: from);
      if (copy) {
        went ? await markShared(sent) : await _deleteFolder(sent.parent);
      }
      if (state != ReplayJobState.failed) error = null;
    } on FileSystemException catch (failure) {
      error = "Couldn't open the share menu: ${failure.message}.";
    } on ShareUnavailable catch (why) {
      if (copy && sent != null) await _deleteFolder(sent.parent);
      error = why.sentence
          ? why.message
          : "Couldn't open the share menu: ${why.message}";
    }
    _changed();
  }

  /// What's left of it here once it's dismissed: on an iPhone or iPad the
  /// copy kept for Share…, once it's saved (in Photos or Files); an unsaved
  /// one only when the person said to delete it.
  Future<void> discard() async {
    final made = file;
    if (made == null || savedTo == ReplaySavedTo.movies) return;
    await _deleteFolder(made.parent);
  }
}

/// [name] in [folder], or "name 2.ext", "name 3.ext"… beside what's there.
File _freeName(Directory folder, String name) {
  final dot = name.lastIndexOf('.');
  final stem = dot > 0 ? name.substring(0, dot) : name;
  final extension = dot > 0 ? name.substring(dot) : '';
  var file = File('${folder.path}/$name');
  for (var n = 2; file.existsSync() || Link(file.path).existsSync(); n++) {
    file = File('${folder.path}/$stem $n$extension');
  }
  return file;
}

Future<void> _deleteFolder(Directory folder) async {
  try {
    if (await folder.exists()) await folder.delete(recursive: true);
  } on FileSystemException {
    // Swept with the other shares.
  }
}

class _Cancelled implements Exception {
  const _Cancelled();
}

/// The videos being made: one at a time.
class ReplayJobs extends ChangeNotifier {
  ReplayJob? _current;

  /// The one being made, or the last one made, until it's dismissed.
  ReplayJob? get current => _current;

  /// Whether one is being made now.
  bool get busy {
    final job = _current;
    return job != null && !job.done;
  }

  /// Starts [job], in place of a finished one still shown.
  void start(ReplayJob job) {
    final before = _current;
    if (before != null) {
      before.removeListener(notifyListeners);
      unawaited(before.discard());
      before.dispose();
    }
    _current = job..addListener(notifyListeners);
    notifyListeners();
    unawaited(
      job.run().then((made) {
        // Cancelled: it goes.
        if (!made && identical(_current, job) && !job.done) dismiss();
      }),
    );
  }

  /// Stops one being made, or puts away one that's done.
  void dismiss() {
    final job = _current;
    if (job == null) return;
    _current = null;
    job.removeListener(notifyListeners);
    if (!job.done) {
      job.cancel();
    } else {
      unawaited(job.discard());
    }
    // Its last frame may still be on its way: disposed after.
    scheduleMicrotask(job.dispose);
    notifyListeners();
  }
}

/// This app's videos being made.
final replayJobs = ReplayJobs();

/// A render tree of its own for the video's frames, off the screen: drawn
/// on demand, never waiting for the display, whatever's on screen.
class _Offscreen {
  _Offscreen(Size size) {
    final dispatcher = WidgetsBinding.instance.platformDispatcher;
    final view = dispatcher.implicitView ?? dispatcher.views.first;
    _view = RenderView(
      view: view,
      configuration: ViewConfiguration(
        logicalConstraints: BoxConstraints.tight(size),
        devicePixelRatio: 1,
      ),
      child: RenderPositionedBox(
        alignment: Alignment.topLeft,
        child: _boundary,
      ),
    );
    _pipeline.rootNode = _view;
    _view.prepareInitialFrame();
  }

  final _boundary = RenderRepaintBoundary();
  final _pipeline = PipelineOwner();
  final _focus = FocusManager();
  late final _build = BuildOwner(focusManager: _focus);
  late final RenderView _view;
  RenderObjectToWidgetElement<RenderBox>? _root;

  /// [widget] drawn: the boundary to take the frame from.
  RenderRepaintBoundary draw(Widget widget) {
    _root = RenderObjectToWidgetAdapter<RenderBox>(
      container: _boundary,
      child: widget,
    ).attachToRenderTree(_build, _root);
    _build
      ..buildScope(_root!)
      ..finalizeTree();
    _pipeline
      ..flushLayout()
      ..flushCompositingBits()
      ..flushPaint();
    return _boundary;
  }

  void dispose() {
    final root = _root;
    if (root != null) {
      RenderObjectToWidgetAdapter<RenderBox>(container: _boundary)
          .attachToRenderTree(_build, root);
      _build
        ..buildScope(root)
        ..finalizeTree();
    }
    _pipeline.rootNode = null;
    _pipeline.dispose();
    _focus.dispose();
  }
}

/// [stage] at [shape]'s pixels, as RGBA bytes.
Future<Uint8List> captureReplayFrame(
  RenderRepaintBoundary stage,
  ReplayShape shape,
) async {
  if (debugReplayCapture case final capture?) return capture(stage, shape);
  final image = await stage.toImage(
    pixelRatio: shape.width / shape.logical.width,
  );
  try {
    final bytes = await image.toByteData(format: ui.ImageByteFormat.rawRgba);
    return bytes!.buffer.asUint8List();
  } finally {
    image.dispose();
  }
}

/// Along the top of the window, wherever the person is, while a video is
/// made and once it's saved: how far it is, with Cancel; then where it is,
/// with Show in Finder and Share…. Nothing while there's none.
class ReplayJobBanner extends StatelessWidget {
  const ReplayJobBanner({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: replayJobs,
      // The same tree whether it shows or not, so what's under it keeps
      // its state as it comes and goes.
      builder: (context, _) {
        final job = replayJobs.current;
        final safe = MediaQuery.paddingOf(context);
        // Out of the way while typing leaves little room, as the device
        // banners are (Sale, row 22).
        final size = MediaQuery.sizeOf(context);
        final keyboard = MediaQuery.viewInsetsOf(context).bottom;
        final aside =
            keyboard > 0 &&
            size.height - keyboard < bannersNeedAbove &&
            isPhoneLayout(context);
        final shown = job != null && !aside;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Container(
              color: shown && isPhoneLayout(context)
                  ? Palette.of(context).canvas
                  : null,
              padding: shown
                  ? EdgeInsets.only(
                      top: safe.top,
                      left: safe.left,
                      right: safe.right,
                    )
                  : EdgeInsets.zero,
              child: job == null || aside
                  ? const SizedBox.shrink()
                  : _JobLine(key: ObjectKey(job), job: job),
            ),
            Expanded(
              child: MediaQuery.removePadding(
                context: context,
                removeTop: shown,
                child: child,
              ),
            ),
          ],
        );
      },
    );
  }
}

class _JobLine extends StatelessWidget {
  const _JobLine({super.key, required this.job});

  final ReplayJob job;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return ListenableBuilder(
      listenable: job,
      builder: (context, _) {
        final error = job.error;
        final (String words, List<Widget> actions) = switch (job.state) {
          ReplayJobState.making => (
            'Making your History video… ${(job.progress * 100).floor()}%',
            [_StripButton(label: 'Cancel', onPressed: replayJobs.dismiss)],
          ),
          ReplayJobState.saving => ('Saving your History video…', const []),
          ReplayJobState.saved => (
            switch (job.savedTo) {
              ReplaySavedTo.photos => 'Your History video is in Photos.',
              ReplaySavedTo.files => 'Your History video is in Files.',
              _ => 'Your History video is in $replayFolderName.',
            },
            [
              if (job.savedTo == ReplaySavedTo.movies)
                _StripButton(
                  label: 'Show in Finder',
                  onPressed: () => showInFinder([job.file!]),
                ),
              Builder(
                builder: (context) => _StripButton(
                  label: 'Share…',
                  onPressed: () => job.share(from: globalRectOf(context)),
                ),
              ),
              _Close(onTap: replayJobs.dismiss),
            ],
          ),
          ReplayJobState.unsaved => (
            "Your History video isn't saved yet.",
            [
              Button(
                label: 'Save to Files…',
                primary: true,
                onPressed: job.saveToFiles,
              ),
              Builder(
                builder: (context) => _StripButton(
                  label: 'Share…',
                  onPressed: () => job.share(from: globalRectOf(context)),
                ),
              ),
              _Close(onTap: () => _confirmDiscard(context)),
            ],
          ),
          ReplayJobState.failed => (
            error == null
                ? "Couldn't make your History video. Try again."
                : "Couldn't make your History video: $error",
            [_Close(onTap: replayJobs.dismiss)],
          ),
        };
        final failed = job.state == ReplayJobState.failed;
        return Semantics(
          container: true,
          liveRegion: job.done,
          child: Container(
            padding: const EdgeInsets.symmetric(
              horizontal: Space.l,
              vertical: Space.s,
            ),
            decoration: BoxDecoration(
              border: Border(bottom: BorderSide(color: palette.hairline)),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                _Line(
                  words: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        words,
                        style: type.body.copyWith(
                          color: failed ? palette.danger : palette.text,
                          fontFeatures: const [ui.FontFeature.tabularFigures()],
                        ),
                      ),
                      if (job.state == ReplayJobState.saved &&
                          job.savedTo == ReplaySavedTo.movies)
                        Text(
                          job.file!.uri.pathSegments.last,
                          style: type.caption.copyWith(
                            color: palette.textMuted,
                          ),
                        ),
                      if (error != null && !failed)
                        Text(
                          error,
                          style: type.caption.copyWith(
                            color: job.state == ReplayJobState.unsaved
                                ? palette.textMuted
                                : palette.danger,
                          ),
                        ),
                    ],
                  ),
                  actions: actions,
                ),
                // The whole width under the words and Cancel (Lucy).
                if (job.state == ReplayJobState.making) ...[
                  const SizedBox(height: Space.xs),
                  ReplayProgressBar(value: job.progress),
                  _FramePixel(job: job),
                ],
              ],
            ),
          ),
        );
      },
    );
  }

  Future<void> _confirmDiscard(BuildContext context) async {
    final yes = await confirmDeleteUnsavedReplay(context);
    if (yes) replayJobs.dismiss();
  }
}

/// Asks before an unsaved video goes: it's nowhere else.
Future<bool> confirmDeleteUnsavedReplay(BuildContext context) async {
  if (isPhoneLayout(context)) {
    return await showActions<bool>(
          context,
          title: "This video isn't saved anywhere.",
          actions: const [SheetAction('Delete video', true, destructive: true)],
          cancelLabel: 'Keep it',
        ) ??
        false;
  }
  var yes = false;
  await showModal(
    context,
    (context, close) => FormFrame(
      title: 'Delete this video?',
      body: "It isn't saved anywhere, so you can't get it back.",
      fields: const [],
      error: null,
      submitLabel: 'Delete',
      onSubmit: () {
        yes = true;
        close();
      },
      onCancel: close,
    ),
  );
  return yes;
}

/// Words with buttons at the end of the line; on a phone, under them.
class _Line extends StatelessWidget {
  const _Line({required this.words, required this.actions});

  final Widget words;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    // Under the words only where they'd be squeezed: on a phone, with the
    // largest text or more than a button and ×.
    if (isPhoneLayout(context) && (largeText(context) || actions.length > 2)) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          words,
          if (actions.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            Wrap(
              alignment: WrapAlignment.end,
              crossAxisAlignment: WrapCrossAlignment.center,
              spacing: Space.s,
              runSpacing: Space.s,
              children: actions,
            ),
          ],
        ],
      );
    }
    return Row(
      children: [
        Expanded(child: words),
        for (final (i, action) in actions.indexed) ...[
          SizedBox(width: i == 0 ? Space.m : Space.s),
          action,
        ],
      ],
    );
  }
}

/// A pixel that changes, unseen, with every frame of the video made: so an
/// iPhone, which draws only what changed, still draws a frame on screen
/// between two of the video's ([ReplayJob._screenFrame]).
class _FramePixel extends StatelessWidget {
  const _FramePixel({required this.job});

  final ReplayJob job;

  @override
  Widget build(BuildContext context) => ExcludeSemantics(
    child: ValueListenableBuilder<int>(
      valueListenable: job.made,
      builder: (context, i, _) => SizedBox(
        width: 1,
        height: 1,
        child: ColoredBox(
          color: Palette.of(context).text.withAlpha(i.isEven ? 1 : 2),
        ),
      ),
    ),
  );
}

/// A strip's button: a quiet text button, a finger tall on a phone, as
/// the Mac's quiet [Button] (Lucy).
class _StripButton extends StatelessWidget {
  const _StripButton({required this.label, required this.onPressed});

  final String label;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    if (!touchFirst(context)) {
      return Button(label: label, onPressed: onPressed);
    }
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Semantics(
      button: true,
      label: label,
      excludeSemantics: true,
      child: Pressable(
        onTap: onPressed,
        builder: (context, pressed) => Container(
          constraints: BoxConstraints(minHeight: Metrics.of(context).target),
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          alignment: Alignment.center,
          child: Text(
            label,
            style: type.body.copyWith(
              color: pressed ? palette.textMuted : palette.text,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
      ),
    );
  }
}

/// The banner's ×.
class _Close extends StatelessWidget {
  const _Close({required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => GlyphButton(
    key: const ValueKey('replay-job-close'),
    label: 'Close',
    active: false,
    onTap: onTap,
    glyph: (color) => ExtensionGlyph(icon: 'x', name: '', color: color),
  );
}

/// A thin bar, filled as far as [value] (0…1).
class ReplayProgressBar extends StatelessWidget {
  const ReplayProgressBar({super.key, required this.value});

  final double value;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return SizedBox(
      height: 4,
      width: double.infinity,
      child: ClipRRect(
        borderRadius: BorderRadius.circular(2),
        child: Stack(
          children: [
            Positioned.fill(child: ColoredBox(color: palette.hover)),
            FractionallySizedBox(
              widthFactor: value.clamp(0.0, 1.0),
              child: ColoredBox(color: palette.signal),
            ),
          ],
        ),
      ),
    );
  }
}
