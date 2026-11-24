// The claktory.com demo film's camera (Kakyoin, Demo Producer): runs the
// real Claktory Dev app against the dev Claktory (port 7421) and saves what
// it draws, frame by frame, as numbered PNGs, while a person (or the
// producer's shell) directs it through a control file. Nothing in the app
// is faked or changed: it's the app as it ships, connected to real agents;
// the frames are its own drawing, as the tour tests save them, so no Screen
// Recording permission is needed and nothing else on the screen is seen.
//
//   cd app && FLUTTER_XCODE_PRODUCT_BUNDLE_IDENTIFIER=dev.factory.app.demo \
//     flutter test integration_test/demo_capture_test.dart -d macos \
//     --dart-define=FACTORY_DEV=true --dart-define=SHOTS_DIR=/tmp/demo \
//     [--dart-define=SCALE=2]
//
// SCALE draws each frame that many times the window's pixels (1 unless
// set): the app's layers drawn again at that size, so text stays sharp
// while the window and its layout stay as they are.
//
// Then append commands to SHOTS_DIR/control, one a line (read each step):
//   fps N        frames a second into SHOTS_DIR/frames (0, the default: none)
//   select ID    open that agent's conversation (by id)
//   page NAME    agent, settings, map, inbox, usage, monitors…
//   type TEXT    types TEXT into the message box, one frame per character
//   send         sends the box's text, as Return in it does
//   file PATH    opens PATH (on the dev machine) beside the conversation
//   shot NAME    saves one frame as SHOTS_DIR/NAME.png
//   region X Y W H  from then on, frames (and shots) hold only that
//                rectangle of the view, in points from its top left: far
//                quicker to take, so a high fps holds. `region` alone goes
//                back to the whole view
//   size W H     lays the app out as a W×H point window from then on (its
//                frames W×H points at the screen's pixel ratio, times
//                SCALE); on a Mac, under 600 points wide is the phone's
//                layout, as a window that narrow would be. `size` alone
//                goes back to the real window
//   scroll TEXT  scrolls the conversation shown to the first (oldest)
//                message with TEXT in it, loading back to it, and puts it
//                near the top: as Needs you goes to a card. Prints
//                "scrolled" with the message's words around TEXT, or
//                "not found: TEXT"
//   voice FILE   records FILE (.m4a) as a voice message from the mic and
//                sends it once it has played; FILE.levels (or backup.levels
//                for backup.m4a) beside it holds
//                its loudness, one level (0 to 1) a line, one line per 50 ms
//   quit         ends the test
// Every frame is logged in SHOTS_DIR/frames.txt with the wall-clock time
// and the agent shown, so the edit can print the real elapsed time.
//
// It only ever films the dev Claktory: it refuses to start without
// FACTORY_DEV (which also picks ~/.factory-dev's own pairing) or on any
// port but 7421, and stops if the app knows any other Claktory (its own
// bundle id keeps the pairings other tests' runs saved out of its reach).
//
// No agent can give the app a microphone, so `voice` stands a file in for
// it (_FileMic): the file is what's recorded, its levels play in real time
// as the microphone's would, and everything after is the real path:
// upload, transcription on the dev Claktory, and the agent's reply.
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/backend.dart' show RequestFailure, localPort;
import 'package:factory_app/explorer.dart' show openFile;
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/message_box.dart' show MessageBox;
import 'package:factory_app/recorder.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/sight.dart' show Sight;
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart' show EditableText;
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _shots = String.fromEnvironment('SHOTS_DIR', defaultValue: '/tmp/demo');
const _dev = bool.fromEnvironment('FACTORY_DEV');
final _scale = double.parse(
  const String.fromEnvironment('SCALE', defaultValue: '1'),
);

/// A file in place of the microphone: what [start] records is [file], and
/// its loudness comes from [levels], one every 50 ms in real time, as the
/// native recorder's does (Recorder.swift's scale), for as long as it runs.
class _FileMic implements VoiceRecorder {
  String file = '';
  List<double> levels = const [];

  /// Done once the last level has played.
  Completer<void> played = Completer<void>();

  final _events = StreamController<RecorderEvent>.broadcast();
  Timer? _timer;
  String? _path;

  static const _every = Duration(milliseconds: 50);

  @override
  Future<MicAccess> access() async => MicAccess.granted;

  @override
  Future<bool> request() async => true;

  @override
  Future<void> openSettings() async {}

  @override
  Future<void> start(String path) async {
    // Into its folder, as the native recorder makes it.
    File(path).parent.createSync(recursive: true);
    File(file).copySync(path);
    _path = path;
    var at = 0;
    _timer = Timer.periodic(_every, (timer) {
      if (at >= levels.length) {
        timer.cancel();
        if (!played.isCompleted) played.complete();
        return;
      }
      _events.add(RecorderEvent(level: levels[at], elapsed: _every * at));
      at++;
    });
  }

  @override
  Future<Recording?> stop() async {
    _timer?.cancel();
    final path = _path;
    _path = null;
    if (path == null) return null;
    return Recording(path: path, duration: _every * levels.length);
  }

  @override
  Future<void> cancel() async {
    _timer?.cancel();
    final path = _path;
    _path = null;
    if (path != null && File(path).existsSync()) File(path).deleteSync();
  }

  @override
  Stream<RecorderEvent> get events => _events.stream;
}

/// [file]'s levels: FILE.levels, or beside it by its name without its
/// extension (backup.m4a, backup.levels).
File _levelsOf(String file) {
  final beside = File('$file.levels');
  if (beside.existsSync()) return beside;
  final dot = file.lastIndexOf('.');
  return File(
    '${dot > file.lastIndexOf('/') ? file.substring(0, dot) : file}.levels',
  );
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  // Frames come from the test, not the screen: a covered window works.
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.benchmark;

  testWidgets('demo capture', timeout: Timeout.none, (tester) async {
    if (!_dev || localPort != '7421') {
      fail(
        'the demo films only the dev Claktory: run it with '
        '--dart-define=FACTORY_DEV=true and no FACTORY_PORT',
      );
    }
    final control = File('$_shots/control');
    await Directory('$_shots/frames').create(recursive: true);
    if (!control.existsSync()) control.writeAsStringSync('');
    final log = File('$_shots/frames.txt').openWrite(mode: FileMode.append);
    // Before the app: each message box takes its recorder as it's made.
    final mic = _FileMic();
    deviceRecorder = mic;
    var read = control.lengthSync();
    // The app's clock is the wall clock, so what moves moves in real time
    // however long a frame takes to save.
    final clock = Stopwatch()..start();
    var fps = 0.0;
    // What each frame holds, in logical points: null for the whole view.
    Rect? region;
    // Frames still being encoded and written, oldest first.
    final writing = <Future<void>>[];
    var frame = Directory('$_shots/frames').listSync().length;
    var lastFrame = DateTime.fromMillisecondsSinceEpoch(0);

    Future<void> step() async {
      // Twice as often as frames are due, so none waits long for its turn.
      final wait = Duration(
        microseconds: fps > 0
            ? (500000 / fps).clamp(5000, 50000).round()
            : 50000,
      );
      await Future<void>.delayed(wait);
      // The camera films a window that may be covered, where the app stops
      // everything that moves (sight.dart): here it's always being seen.
      Sight.shown.value = true;
      await tester.pumpBenchmark(clock.elapsed);
    }

    /// The view as drawn now ([region] of it, if set), to save as [path]
    /// once it's encoded: the returned future.
    Future<Future<void>> take(String path) async {
      final view = binding.renderViews.first;
      final ratio = view.flutterView.devicePixelRatio;
      final shown = region ?? Offset.zero & view.size;
      // The view's layer is already in physical pixels; SCALE draws it
      // larger still.
      final image = await (view.debugLayer! as OffsetLayer).toImage(
        Rect.fromLTWH(
          shown.left * ratio,
          shown.top * ratio,
          shown.width * ratio,
          shown.height * ratio,
        ),
        pixelRatio: _scale,
      );
      return () async {
        final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
        image.dispose();
        await File(path).writeAsBytes(bytes!.buffer.asUint8List());
      }();
    }

    Future<void> save(String path) async => await take(path);

    late AppScope scope;
    EditableText messageBox() => tester.widget<EditableText>(
      find
          .descendant(
            of: find.byType(MessageBox),
            matching: find.byType(EditableText),
          )
          .last,
    );
    Future<void> grab() async {
      frame += 1;
      final name = frame.toString().padLeft(6, '0');
      // Logged at the moment it shows, not when it's written.
      lastFrame = DateTime.now();
      final shown = scope.backend.workspace.workspace.selectedAgentId;
      log.writeln('$name ${lastFrame.toIso8601String()} $shown');
      // Encoded and written while the next frames are taken; a few at most
      // at once.
      writing.add(await take('$_shots/frames/$name.png'));
      while (writing.length > 6) {
        await writing.removeAt(0);
      }
    }

    /// A step, and a frame when one is due at [fps].
    Future<void> tick() async {
      await step();
      if (fps > 0 &&
          DateTime.now().difference(lastFrame).inMilliseconds >= 1000 / fps) {
        await grab();
      }
    }

    /// The message box's own button labelled [label], pressed through its
    /// handler.
    VoidCallback? pressIn(String label) {
      final found = find.descendant(
        of: find.byType(MessageBox),
        matching: find.byWidgetPredicate(
          (widget) => widget is Button && widget.label == label,
        ),
      );
      if (found.evaluate().isEmpty) return null;
      return tester.widget<Button>(found.last).onPressed;
    }

    await app.main();
    final start = DateTime.now();
    while (find.byType(Sidebar).evaluate().isEmpty) {
      if (DateTime.now().difference(start) > const Duration(minutes: 2)) {
        fail('the app never showed its sidebar');
      }
      await step();
    }
    scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    void onlyDev() {
      final connections = scope.connections;
      if (connections.current != null || connections.saved.isNotEmpty) {
        fail(
          'this app knows another Claktory besides this Mac\'s dev one: '
          'run it under its own bundle id, dev.factory.app.demo',
        );
      }
    }

    onlyDev();
    // The sidebar shows before the connection is up; commands wait for it.
    while (!scope.backend.connected.value) {
      if (DateTime.now().difference(start) > const Duration(minutes: 2)) {
        fail('the app never connected to the dev Claktory');
      }
      await step();
    }
    // ignore: avoid_print
    print('demo capture ready: append commands to ${control.path}');

    while (true) {
      await step();
      onlyDev();
      final length = control.lengthSync();
      if (length > read) {
        final raf = control.openSync();
        raf.setPositionSync(read);
        final text = String.fromCharCodes(raf.readSync(length - read));
        raf.closeSync();
        read = length;
        for (final line in text.split('\n')) {
          final command = line.trim();
          if (command.isEmpty) continue;
          final space = command.indexOf(' ');
          final verb = space < 0 ? command : command.substring(0, space);
          final arg = space < 0 ? '' : command.substring(space + 1);
          log.writeln('# ${DateTime.now().toIso8601String()} $command');
          switch (verb) {
            case 'quit':
              await Future.wait(writing);
              await log.close();
              return;
            case 'fps':
              fps = double.parse(arg);
            case 'select':
              await scope.backend.selectAgent(arg);
              scope.page.value = AppPage.agent;
              // Until its conversation, and its own message box, show.
              for (var i = 0; i < 200; i++) {
                await step();
                if (i >= 5 &&
                    scope.backend.workspace.workspace.selectedAgentId == arg) {
                  break;
                }
              }
            case 'region':
              final xywh = [
                for (final part in arg.split(' '))
                  if (part.isNotEmpty) double.parse(part),
              ];
              region = xywh.length == 4
                  ? Rect.fromLTWH(xywh[0], xywh[1], xywh[2], xywh[3])
                  : null;
            case 'size':
              // The view as the app sees it (MediaQuery and layout alike);
              // the real window keeps its size, and the frames are the
              // view's own drawing, so they come out at the new size.
              final wh = arg.split(' ').where((p) => p.isNotEmpty).toList();
              if (wh.isEmpty) {
                tester.view.resetPhysicalSize();
              } else {
                tester.view.physicalSize =
                    Size(double.parse(wh[0]), double.parse(wh[1])) *
                    tester.view.devicePixelRatio;
              }
              for (var i = 0; i < 5; i++) {
                await step();
              }
            case 'scroll':
              // Claktory's own search of the whole conversation, newest
              // first, a page at a time: the last match is the oldest.
              final backend = scope.backend;
              final agentId = backend.workspace.workspace.selectedAgentId;
              ({int seq, String snippet})? first;
              int? before;
              try {
                while (true) {
                  final found = await backend.searchConversation(
                    agentId: agentId,
                    query: arg,
                    beforeSeq: before,
                  );
                  if (found.matches.isEmpty) break;
                  final last = found.matches.last;
                  first = (seq: last.seq.toInt(), snippet: last.snippet);
                  if (!found.more) break;
                  before = first.seq;
                }
              } on RequestFailure catch (error) {
                // ignore: avoid_print
                print('not found: $arg (${error.message})');
                continue;
              }
              if (first == null) {
                // ignore: avoid_print
                print('not found: $arg');
                continue;
              }
              backend.reveal.reachSeq(agentId, first.seq);
              // Until the conversation has gone there (loading back to it
              // first), and laid out there.
              for (var i = 0; i < 200 && backend.reveal.value != null; i++) {
                await step();
              }
              for (var i = 0; i < 5; i++) {
                await step();
              }
              // ignore: avoid_print
              print('scrolled: ${first.snippet}');
            case 'page':
              scope.page.value = AppPage.values.byName(arg);
            case 'file':
              openFile(tester.element(find.byType(Sidebar).first), arg);
            case 'shot':
              for (var i = 0; i < 5; i++) {
                await step();
              }
              await save('$_shots/$arg.png');
            // Neither goes through the tester's input helpers: they pump
            // frames themselves, which never come under the benchmark
            // policy, so they'd wait forever.
            case 'type':
              messageBox().focusNode.requestFocus();
              final runes = arg.runes.toList();
              for (var i = 1; i <= runes.length; i++) {
                final text = String.fromCharCodes(runes.take(i));
                messageBox().controller.value = TextEditingValue(
                  text: text,
                  selection: TextSelection.collapsed(offset: text.length),
                );
                await step();
                await grab();
              }
            case 'voice':
              mic
                ..file = arg
                ..levels = [
                  for (final line in _levelsOf(arg).readAsLinesSync())
                    if (line.trim().isNotEmpty) double.parse(line),
                ];
              // The mic, through its own handler.
              final mics = find.descendant(
                of: find.byType(MessageBox),
                matching: find.byWidgetPredicate(
                  (widget) => widget.runtimeType.toString() == '_MicButton',
                ),
              );
              mic.played = Completer<void>();
              var done = false;
              unawaited(mic.played.future.then((_) => done = true));
              (tester.widget(mics.last) as dynamic).onTap();
              // Recording, for as long as the file runs.
              final until = DateTime.now().add(
                const Duration(milliseconds: 50) * mic.levels.length +
                    const Duration(seconds: 30),
              );
              while (!done || pressIn('Send') == null) {
                if (DateTime.now().isAfter(until)) break;
                await tick();
              }
              final send = pressIn('Send');
              if (send == null) {
                // ignore: avoid_print
                print("voice didn't record: no Send in the message box");
              } else {
                send();
                await tick();
                // ignore: avoid_print
                print('voice sent');
              }
            case 'send':
              // Return in the box, as a person sends on a Mac: the box's
              // own key handler, which sends what's in it.
              final node = messageBox().focusNode;
              node.onKeyEvent!(
                node,
                const KeyDownEvent(
                  physicalKey: PhysicalKeyboardKey.enter,
                  logicalKey: LogicalKeyboardKey.enter,
                  timeStamp: Duration.zero,
                ),
              );
          }
        }
      }
      if (fps > 0 &&
          DateTime.now().difference(lastFrame).inMilliseconds >= 1000 / fps) {
        await grab();
      }
    }
  });
}
