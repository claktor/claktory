// Voice messages in the app (#197): recording one in an agent's message
// box (the waveform as it's heard, a timer, Cancel), which the composer
// then sends as a voice message for the agent's Claktory to transcribe;
// asking once before that Claktory downloads its speech model; how a voice
// message shows while it's transcribed, if it fails, and once it's text
// (with ▶ to play it back); and Settings → Voice. The recording and
// playing are native (recorder.dart, audio.dart).
import 'dart:async';
import 'dart:io' show File, FileSystemException;
import 'dart:math' as math;

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'audio.dart';
import 'backend.dart';
import 'claktory_identity.dart' show claktoryName;
import 'controls.dart';
import 'forms.dart' show FormFrame;
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'recorder.dart';
import 'resources.dart' show formatBytes;
import 'scope.dart';
import 'mobile/metrics.dart';
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'settings_view.dart' show SettingRequests, SettingRow;
import 'usage.dart' show formatAge;
import 'voice_keep.dart';

/// Where to allow the microphone once it's refused for good, on this
/// platform: the Settings page Open Settings opens beside it.
String micRefusedForGood() => switch (defaultTargetPlatform) {
  TargetPlatform.macOS =>
    "Claktory can't use the microphone. Allow it in System Settings → "
        'Privacy & Security → Microphone.',
  TargetPlatform.iOS =>
    'To record, allow microphone access in Settings → Claktory.',
  TargetPlatform.android =>
    'To record, allow Claktory to use the microphone in Settings.',
  _ => "Claktory can't use the microphone. Allow it in this device's settings.",
};

/// One recording in an agent's message box, from the mic button: asked
/// for the microphone, then recording, its loudness kept for the waveform,
/// until it's finished (to send) or cancelled.
class VoiceRecording extends ChangeNotifier {
  VoiceRecording({VoiceRecorder? recorder, this.now = DateTime.now})
    : _mic = recorder ?? deviceRecorder;

  final VoiceRecorder _mic;

  /// The time now, for the file's name.
  final DateTime Function() now;

  /// How many loudness samples the waveform keeps: the last few seconds.
  static const samples = 120;

  var _recording = false;
  var _starting = false;
  var _elapsed = Duration.zero;
  final _levels = <double>[];
  String? _error;
  StreamSubscription<RecorderEvent>? _events;

  /// Recording now.
  bool get recording => _recording;

  /// Asking for the microphone, or starting it.
  bool get starting => _starting;

  Duration get elapsed => _elapsed;

  /// The loudness heard, oldest first, 0 to 1.
  List<double> get levels => List.unmodifiable(_levels);

  /// Why it couldn't record, for the person.
  String? get error => _error;

  /// The recording ended by itself: at the time limit (kept, to send), or
  /// interrupted (kept), or failed (gone).
  void Function(RecordingEnd end)? onEnded;

  /// Starts recording, asking for the microphone first if it hasn't been;
  /// false, with [error] set, if it can't.
  Future<bool> start() async {
    if (_recording || _starting) return _recording;
    _starting = true;
    _error = null;
    notifyListeners();
    try {
      var access = await _mic.access();
      if (access == MicAccess.undetermined) {
        // Refused, it's asked again rather than assumed for good: Android
        // can still ask after a first no (undetermined), and only a
        // second one, or an iPhone's or a Mac's, is for good (denied).
        access = await _mic.request() ? MicAccess.granted : await _mic.access();
      }
      switch (access) {
        case MicAccess.granted:
          break;
        case MicAccess.unavailable:
          _error = "This device can't record voice messages.";
          return false;
        case MicAccess.undetermined:
          _error =
              'Claktory needs the microphone to record. Tap the microphone '
              'again to allow it.';
          return false;
        case MicAccess.restricted:
          _error =
              "This device's restrictions don't allow Claktory to use the "
              'microphone.';
          return false;
        case MicAccess.denied:
          _error = micRefusedForGood();
          return false;
      }
      _levels.clear();
      _elapsed = Duration.zero;
      // Straight into the folder it's kept in until sent (#401).
      final into = await newRecordingPath(now());
      _into = into;
      // What earlier recordings left behind unknown to the app goes now.
      unawaited(sweepEmptyRecordings());
      _events = _mic.events.listen(_heard);
      try {
        await _mic.start(into.path);
      } on RecordingFailure catch (failure) {
        await _events?.cancel();
        _events = null;
        _error = failure.message;
        _dropFolder();
        return false;
      }
      if (_disposed) {
        // Its message box went while the microphone started.
        await _mic.cancel();
        _dropFolder();
        return false;
      }
      _recording = true;
      return true;
    } finally {
      _starting = false;
      if (!_disposed) notifyListeners();
    }
  }

  void _heard(RecorderEvent event) {
    if (!_recording) return;
    final ended = event.ended;
    if (ended != null) {
      onEnded?.call(ended);
      return;
    }
    _elapsed = event.elapsed;
    _levels.add(event.level);
    if (_levels.length > samples) _levels.removeAt(0);
    notifyListeners();
  }

  /// Stops, keeping what was recorded; null if nothing was.
  Future<Recording?> finish() async {
    if (!_recording) return null;
    _recording = false;
    final events = _events;
    _events = null;
    notifyListeners();
    await events?.cancel();
    final made = await _mic.stop();
    // Nothing in it (too short for the microphone to make a file of): its
    // folder goes, as a cancelled one's, and the caller says so.
    if (made == null || !_hasSound(made.path)) _dropFolder();
    return made;
  }

  static bool _hasSound(String path) {
    final file = File(path);
    return file.existsSync() && file.lengthSync() > 0;
  }

  /// Stops and throws the recording away.
  Future<void> cancel() async {
    if (!_recording) return;
    _recording = false;
    final events = _events;
    _events = null;
    notifyListeners();
    await events?.cancel();
    await _mic.cancel();
    _dropFolder();
  }

  /// Where the recording goes, and whether that's the kept folder.
  ({String path, bool kept})? _into;

  /// Whether the last recording was made in the folder recordings are
  /// kept in until sent.
  bool get kept => _into?.kept ?? false;

  /// A recording thrown away: its own folder in the kept one goes too.
  void _dropFolder() {
    final into = _into;
    if (into == null || !into.kept) return;
    try {
      final folder = File(into.path).parent;
      if (folder.existsSync()) folder.deleteSync(recursive: true);
    } on FileSystemException {
      // Swept at the next launch.
    }
  }

  var _disposed = false;

  @override
  void dispose() {
    _disposed = true;
    // Gone with its message box.
    if (_recording) {
      _recording = false;
      unawaited(_mic.cancel().then((_) => _dropFolder()));
    }
    unawaited(_events?.cancel());
    _events = null;
    super.dispose();
  }
}

/// "0:42", "10:00".
String voiceDuration(Duration duration) {
  final seconds = duration.inSeconds;
  return '${seconds ~/ 60}:${(seconds % 60).toString().padLeft(2, '0')}';
}

/// A voice note waiting in a message box (#425), recorded while the box
/// had something written: a chip over the text, as a file's is, with the
/// mic, "Voice note" and its length, and × to take it out. It goes with
/// the text when the box is sent.
class VoiceNoteChip extends StatelessWidget {
  const VoiceNoteChip({
    super.key,
    required this.duration,
    required this.onRemove,
  });

  final Duration duration;
  final VoidCallback onRemove;

  static const label = 'Voice note';

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    // On a phone, a finger's size, its × too, as a file's chip.
    final phone = isPhoneLayout(context);
    return Container(
      height: phone ? Touch.target : 28,
      decoration: BoxDecoration(
        color: palette.canvas,
        borderRadius: BorderRadius.circular(Radii.control),
        border: Border.all(color: palette.hairline),
      ),
      padding: const EdgeInsets.only(left: Space.s, right: 2),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Semantics(
            container: true,
            label: '$label, ${voiceDuration(duration)}',
            excludeSemantics: true,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                ExtensionGlyph(
                  icon: 'mic',
                  name: label,
                  color: palette.textMuted,
                  size: 14,
                ),
                const SizedBox(width: Space.xs + 2),
                Text(
                  label,
                  maxLines: 1,
                  style: (phone ? type.secondary : TextStyles.label).copyWith(
                    color: palette.text,
                  ),
                ),
                const SizedBox(width: Space.xs + 2),
                Text(
                  voiceDuration(duration),
                  style: type.caption.copyWith(
                    color: palette.textFaint,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
              ],
            ),
          ),
          Pressable(
            onTap: onRemove,
            builder: (context, hovered) => Semantics(
              container: phone,
              button: true,
              label: 'Remove voice note',
              onTap: phone ? onRemove : null,
              child: SizedBox.square(
                dimension: phone ? Touch.target : 24,
                child: Center(
                  child: ExtensionGlyph(
                    icon: 'x',
                    name: 'Remove',
                    color: hovered ? palette.text : palette.textFaint,
                    size: phone ? 16 : 12,
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// What shows in the message box's place while recording: a live dot, the
/// waveform as it's heard, the time, and Cancel.
class RecordingStrip extends StatelessWidget {
  const RecordingStrip({
    super.key,
    required this.voice,
    required this.onCancel,
  });

  final VoiceRecording voice;
  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: voice,
      builder: (context, _) => Semantics(
        label: 'Recording, ${voiceDuration(voice.elapsed)}',
        child: Row(
          children: [
            LiveDot(color: palette.danger),
            const SizedBox(width: Space.s),
            Expanded(
              child: SizedBox(
                height: 20,
                child: CustomPaint(
                  painter: Waveform(voice.levels, color: palette.textMuted),
                ),
              ),
            ),
            const SizedBox(width: Space.s),
            Text(
              voiceDuration(voice.elapsed),
              style: TextStyles.caption.copyWith(
                color: palette.textMuted,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
            const SizedBox(width: Space.s),
            Button(label: 'Cancel', onPressed: onCancel),
          ],
        ),
      ),
    );
  }
}

/// Loudness as bars, 2px wide with 2px between, centered on the middle,
/// the newest at the right end; at least a dot where it's silent.
class Waveform extends CustomPainter {
  Waveform(this.levels, {required this.color});

  final List<double> levels;
  final Color color;

  static const _bar = 2.0;
  static const _gap = 2.0;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()
      ..color = color
      ..strokeWidth = _bar
      ..strokeCap = StrokeCap.round;
    final fits = (size.width / (_bar + _gap)).floor();
    final shown = levels.length > fits
        ? levels.sublist(levels.length - fits)
        : levels;
    final middle = size.height / 2;
    for (var i = 0; i < shown.length; i++) {
      final x = size.width - (shown.length - i) * (_bar + _gap) + _bar / 2;
      final half = math.max(0.5, shown[i] * middle);
      canvas.drawLine(
        Offset(x, middle - half),
        Offset(x, middle + half),
        paint,
      );
    }
  }

  @override
  bool shouldRepaint(Waveform old) =>
      old.levels != levels || old.color != color;
}

/// Whether voice messages are offered at all (the mic, Settings → Voice);
/// on since Claktory transcribes them (#197 slice 3). Tests turn it off.
bool voiceMessagesOn = true;

/// Whether [workspace]'s Claktory knows voice messages: an older one
/// says nothing of a speech model, and then the mic (and Settings → Voice)
/// isn't offered.
bool voiceKnown(Workspace workspace) =>
    voiceMessagesOn &&
    workspace.hasSpeechModel() &&
    workspace.speechModel.state !=
        SpeechModelState.SPEECH_MODEL_STATE_UNSPECIFIED;

/// Why voice messages can't be sent to [model]'s Claktory at all, for the
/// person; null when they can (once its model is there).
String? voiceUnavailable(SpeechModel model) =>
    model.state == SpeechModelState.SPEECH_MODEL_STATE_UNSUPPORTED
    ? (model.error.isEmpty
          ? "The computer this Claktory runs on can't recognize speech."
          : model.error)
    : null;

/// "312 of 740 MB": [bytes] in [total]'s unit.
String bytesOf(Int64 bytes, Int64 total) {
  final whole = formatBytes(total);
  final space = whole.lastIndexOf(' ');
  final unit = whole.substring(space + 1);
  const scale = {'KB': 1e3, 'MB': 1e6, 'GB': 1e9, 'TB': 1e12};
  final divisor = scale[unit];
  if (divisor == null) return '${formatBytes(bytes)} of $whole';
  final part = bytes.toDouble() / divisor;
  final digits = whole.substring(0, space).contains('.') ? 1 : 0;
  return '${part.toStringAsFixed(digits)} of $whole';
}

/// What a voice message waiting for [model] says: its download, or that
/// it's being transcribed.
String transcribingText(SpeechModel model) => switch (model.state) {
  SpeechModelState.SPEECH_MODEL_STATE_DOWNLOADING when model.total > 0 =>
    'Downloading the speech model… ${bytesOf(model.bytes, model.total)}',
  SpeechModelState.SPEECH_MODEL_STATE_DOWNLOADING ||
  SpeechModelState.SPEECH_MODEL_STATE_VERIFYING ||
  SpeechModelState.SPEECH_MODEL_STATE_ABSENT ||
  SpeechModelState.SPEECH_MODEL_STATE_FAILED => 'Preparing the speech model…',
  _ => 'Transcribing…',
};

/// Before recording: makes sure the Claktory's speech model is there or on
/// its way, asking the person the first time before it's downloaded (it's
/// big, and often on a computer they don't see). A download that failed
/// is picked up again without asking. False if they'd rather not, or it
/// can't be had.
Future<bool> readyForVoice(BuildContext context) async {
  final backend = AppScope.of(context);
  final model = backend.workspace.workspace.speechModel;
  switch (model.state) {
    case SpeechModelState.SPEECH_MODEL_STATE_READY:
    case SpeechModelState.SPEECH_MODEL_STATE_DOWNLOADING:
    case SpeechModelState.SPEECH_MODEL_STATE_VERIFYING:
      return true;
    case SpeechModelState.SPEECH_MODEL_STATE_FAILED:
      unawaited(
        backend.installSpeechModel().catchError(
          (Object _) {},
          test: (error) => error is RequestFailure,
        ),
      );
      return true;
    case SpeechModelState.SPEECH_MODEL_STATE_ABSENT:
      var agreed = false;
      await showModal(
        context,
        (context, close) => SpeechModelConsent(
          model: model,
          machine: claktoryName(backend),
          onDownload: () {
            agreed = true;
            close();
          },
          close: close,
        ),
      );
      return agreed;
    default:
      return false;
  }
}

/// The one-time question before the speech model downloads.
class SpeechModelConsent extends StatefulWidget {
  const SpeechModelConsent({
    super.key,
    required this.model,
    required this.machine,
    required this.onDownload,
    required this.close,
  });

  final SpeechModel model;

  /// The Claktory's name, where it goes.
  final String machine;

  /// Downloading has started.
  final VoidCallback onDownload;
  final VoidCallback close;

  @override
  State<SpeechModelConsent> createState() => _SpeechModelConsentState();
}

class _SpeechModelConsentState extends State<SpeechModelConsent> {
  String? _error;
  var _starting = false;

  Future<void> _download() async {
    setState(() {
      _starting = true;
      _error = null;
    });
    try {
      await AppScope.of(context).installSpeechModel();
      widget.onDownload();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _starting = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final size = widget.model.total > 0
        ? ' (${formatBytes(widget.model.total)})'
        : '';
    return FormFrame(
      title: 'Download the speech model?',
      body:
          'Voice messages need a speech model$size on ${widget.machine}. '
          "It's downloaded once.",
      fields: const [],
      error: _error,
      submitLabel: 'Download',
      onSubmit: _starting ? null : _download,
      onCancel: widget.close,
    );
  }
}

/// The length of a voice message, its status beside it: in the person's
/// bubble while it waits to be transcribed (or failed).
class VoiceWaiting extends StatelessWidget {
  const VoiceWaiting({super.key, required this.duration});

  /// Zero until the Claktory has read the recording.
  final Duration duration;

  @override
  Widget build(BuildContext context) {
    const white = Color(0xFFFFFFFF);
    final length = duration == Duration.zero
        ? 'Voice message'
        : voiceDuration(duration);
    return Semantics(
      label: duration == Duration.zero ? length : 'Voice message, $length',
      child: ExcludeSemantics(
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            const ExtensionGlyph(
              icon: 'mic',
              name: 'Voice message',
              color: white,
              size: 14,
            ),
            const SizedBox(width: Space.xs),
            Text(
              length,
              style: TextStyles.body.copyWith(
                color: white,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Under a voice message waiting for the agent: "Transcribing…", or the
/// speech model's download it waits for.
class TranscribingLine extends StatelessWidget {
  const TranscribingLine({super.key});

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final workspace = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: workspace,
      builder: (context, _) => Text(
        transcribingText(workspace.workspace.speechModel),
        style: TextStyles.caption.copyWith(color: palette.textFaint),
      ),
    );
  }
}

/// Under a voice message that couldn't be transcribed: why, Retry and
/// Delete.
class VoiceFailed extends StatefulWidget {
  const VoiceFailed({super.key, required this.agentId, required this.message});

  final String agentId;
  final QueuedMessage message;

  @override
  State<VoiceFailed> createState() => _VoiceFailedState();
}

class _VoiceFailedState extends State<VoiceFailed> {
  String? _error;

  Future<void> _run(Future<void> Function(Backend backend) request) async {
    setState(() => _error = null);
    try {
      await request(AppScope.of(context));
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final transcription = widget.message.transcription;
    final id = widget.message.id;
    Widget action(String label, VoidCallback onTap) => Pressable(
      onTap: onTap,
      builder: (context, hovered) => Text(
        label,
        style: TextStyles.caption.copyWith(
          color: hovered ? palette.text : palette.textMuted,
          fontWeight: FontWeight.w500,
        ),
      ),
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        Wrap(
          alignment: WrapAlignment.end,
          crossAxisAlignment: WrapCrossAlignment.center,
          spacing: Space.s,
          children: [
            Text(
              transcription.error.isEmpty
                  ? "Couldn't transcribe this."
                  : transcription.error,
              style: TextStyles.caption.copyWith(color: palette.danger),
            ),
            action(
              'Retry',
              () => _run(
                (backend) => backend.retryVoiceMessage(widget.agentId, id),
              ),
            ),
            action(
              'Delete',
              () => _run(
                (backend) => backend.discardVoiceMessage(widget.agentId, id),
              ),
            ),
          ],
        ),
        if (_error case final error?)
          Text(
            error,
            style: TextStyles.caption.copyWith(color: palette.danger),
          ),
      ],
    );
  }
}

/// ▶ and the length, in a voice message's bubble under what was heard: it
/// plays the recording ([path], on the Claktory's computer), or pauses it.
class VoicePlay extends StatelessWidget {
  const VoicePlay({
    super.key,
    required this.path,
    required this.duration,
    this.player,
  });

  final String path;
  final Duration duration;

  /// The app's [voicePlayer] unless given.
  final VoicePlayer? player;

  @override
  Widget build(BuildContext context) {
    const white = Color(0xFFFFFFFF);
    final player = this.player ?? voicePlayer;
    return ListenableBuilder(
      listenable: player,
      builder: (context, _) {
        final current = player.current == path;
        final playing = current && player.playing;
        final shown = current && player.position > Duration.zero
            ? player.position
            : duration;
        final error = !current ? null : player.error;
        return Semantics(
          button: true,
          label: playing
              ? 'Pause the recording'
              : 'Play the recording, ${voiceDuration(duration)}',
          child: Pressable(
            onTap: () => player.toggle(AppScope.of(context), path),
            builder: (context, hovered) => Opacity(
              opacity: hovered ? 1 : 0.8,
              child: ExcludeSemantics(
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    current && player.fetching
                        ? const SizedBox.square(
                            dimension: 12,
                            child: Center(child: LiveDot(color: white)),
                          )
                        : ExtensionGlyph(
                            icon: playing ? 'pause' : 'play',
                            name: playing ? 'Pause' : 'Play',
                            color: white,
                            size: 12,
                          ),
                    const SizedBox(width: Space.xs),
                    Text(
                      error ?? voiceDuration(shown),
                      style: TextStyles.caption.copyWith(
                        color: white,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

/// Settings → Voice: the speech model voice messages are transcribed with,
/// on the Claktory's computer: its state, downloading or removing it, and
/// its licence's credit.
class VoiceSettings extends StatefulWidget {
  const VoiceSettings({super.key, required this.model});

  final SpeechModel model;

  @override
  State<VoiceSettings> createState() => _VoiceSettingsState();
}

class _VoiceSettingsState extends State<VoiceSettings> with SettingRequests {
  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final model = widget.model;
    final name = model.name.isEmpty ? 'The speech model' : model.name;
    final machine = claktoryName(backend);
    final (String status, List<Widget> actions) = switch (model.state) {
      SpeechModelState.SPEECH_MODEL_STATE_READY => (
        '$name, ${formatBytes(model.bytes)}, on $machine.',
        [
          Button(
            label: 'Remove',
            onPressed: () => run(backend.removeSpeechModel),
          ),
        ],
      ),
      SpeechModelState.SPEECH_MODEL_STATE_DOWNLOADING => (
        model.total > 0
            ? 'Downloading $name… ${bytesOf(model.bytes, model.total)}'
            : 'Downloading $name…',
        [
          Button(
            label: 'Stop',
            onPressed: () => run(backend.removeSpeechModel),
          ),
        ],
      ),
      SpeechModelState.SPEECH_MODEL_STATE_VERIFYING => (
        'Checking the download of $name…',
        const <Widget>[],
      ),
      SpeechModelState.SPEECH_MODEL_STATE_FAILED => (
        model.error.isEmpty
            ? "Couldn't download $name."
            : "Couldn't download $name: ${model.error}",
        [
          Button(
            label: 'Try again',
            primary: true,
            onPressed: () => run(backend.installSpeechModel),
          ),
          Button(
            label: 'Remove',
            onPressed: () => run(backend.removeSpeechModel),
          ),
        ],
      ),
      SpeechModelState.SPEECH_MODEL_STATE_UNSUPPORTED => (
        voiceUnavailable(model)!,
        const <Widget>[],
      ),
      _ => (
        model.total > 0
            ? 'Voice messages need $name (${formatBytes(model.total)}) on '
                  '$machine.'
            : 'Voice messages need $name on $machine.',
        [
          Button(
            label: 'Download',
            onPressed: () => run(backend.installSpeechModel),
          ),
        ],
      ),
    };
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SettingRow(
          label: 'Speech model',
          top: true,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                status,
                style: TextStyles.body.copyWith(color: palette.text),
              ),
              if (actions.isNotEmpty) ...[
                const SizedBox(height: Space.s),
                Wrap(spacing: Space.s, runSpacing: Space.s, children: actions),
              ],
              if (model.attribution.isNotEmpty) ...[
                const SizedBox(height: Space.xs),
                Text(
                  model.attribution,
                  style: TextStyles.caption.copyWith(color: palette.textFaint),
                ),
              ],
              failureLine(palette),
            ],
          ),
        ),
        const KeptRecordings(),
      ],
    );
  }
}

/// Settings → Voice: recordings said for an answer and not sent, kept on
/// this device (#401), whose card has gone (answered elsewhere, say) or
/// isn't open now: each to play, or delete. One whose card opens goes back
/// to it, and is sent.
class KeptRecordings extends StatelessWidget {
  const KeptRecordings({super.key, this.now = DateTime.now});

  final DateTime Function() now;

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final voices = KeptVoices.of(backend);
    return ListenableBuilder(
      listenable: Listenable.merge([voices, voicePlayer]),
      builder: (context, _) {
        final loose = voices.loose;
        if (loose.isEmpty) return const SizedBox.shrink();
        final palette = Palette.of(context);
        return Padding(
          padding: const EdgeInsets.only(top: Space.m),
          child: SettingRow(
            label: 'Not sent',
            top: true,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  "Answers you said that haven't been sent. Open the "
                  'question again to send one, or delete it.',
                  style: TextStyles.caption.copyWith(color: palette.textMuted),
                ),
                for (final voice in loose) ...[
                  const SizedBox(height: Space.s),
                  _KeptRow(key: ValueKey(voice.path), voice: voice, now: now()),
                ],
              ],
            ),
          ),
        );
      },
    );
  }
}

/// A kept recording: ▶, what it was for, its length and age, and Delete.
class _KeptRow extends StatelessWidget {
  const _KeptRow({super.key, required this.voice, required this.now});

  final KeptVoice voice;
  final DateTime now;

  Future<void> _delete(BuildContext context) async {
    final backend = AppScope.of(context);
    final sure = await showActions<bool>(
      context,
      title: 'Delete this recording?',
      message: "It hasn't been sent. This can't be undone.",
      actions: const [SheetAction('Delete', true, destructive: true)],
    );
    if (sure != true) return;
    if (voicePlayer.current == voice.path) await voicePlayer.stop();
    await KeptVoices.of(backend).forget(voice);
    if (backend.connected.value) {
      fireAndLog(backend.cancelUpload(voice.uploadId));
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final playing = voicePlayer.current == voice.path && voicePlayer.playing;
    final label = voice.label.isEmpty ? 'A recording' : voice.label;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        GlyphButton(
          label: playing ? 'Pause' : 'Play',
          active: false,
          onTap: () => unawaited(
            voicePlayer.toggleHere(AppScope.of(context), voice.path),
          ),
          glyph: (color) => ExtensionGlyph(
            icon: playing ? 'pause' : 'play',
            name: playing ? 'Pause' : 'Play',
            color: color,
          ),
        ),
        const SizedBox(width: Space.s),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(label, style: TextStyles.body.copyWith(color: palette.text)),
              Text(
                '${voiceDuration(voice.duration)} · '
                '${formatAge(voice.made.millisecondsSinceEpoch, now)}',
                style: TextStyles.caption.copyWith(
                  color: palette.textMuted,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ),
        ),
        const SizedBox(width: Space.s),
        Button(label: 'Delete', onPressed: () => unawaited(_delete(context))),
      ],
    );
  }
}
