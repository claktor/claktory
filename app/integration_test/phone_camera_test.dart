// The claktory.com film's phone camera (Kakyoin, Demo Producer): the real
// Claktory Dev iPhone app against a real Claktory, directed through a
// control file while the Simulator is recorded (`simctl io recordVideo`)
// or captured (`simctl io screenshot`). Nothing in the app is faked: it's
// the app as it ships, with real agents; only `voice` stands a file in for
// the microphone, which no agent can give the Simulator.
//
// It pairs with MARKS_DIR/code when it isn't paired yet (allow the device
// on the Claktory while it waits), then writes MARKS_DIR/ready and follows
// the commands appended to MARKS_DIR/control, one a line, each done the
// way a person would on the phone:
//   project ID   the drawer, on that project's agents (from All projects)
//   select ID    that agent's conversation, from its row in the drawer
//   voice FILE   a voice message: the mic tapped, FILE (.m4a, on this Mac)
//                recorded in place of the microphone while FILE.levels
//                (or the same name with .levels) plays its loudness, one
//                level (0 to 1) a line per 50 ms, then Send
//   hold         nothing: the screen stays as it is
//   back         Back, or the drawer closed
//   inbox        Needs you: the conversation header's button, or the
//                drawer's row
//   open TITLE   in Needs you, the card whose title has TITLE in it,
//                opened full screen
//   scroll TEXT  scrolls the conversation shown to the first (oldest)
//                message with TEXT in it, loading back to it, and puts it
//                near the top, as the Mac camera does: logs "scrolled:"
//                with the message's words around TEXT, or "not found:"
//   tap TEXT     taps the last widget whose text has TEXT in it, scrolled
//                into view first: logs "tapped:" or "not found:"
//   swipe N [X Y]  drags up N points (down, negative) from screen point
//                X Y (the centre if left out), as a thumb scrolls, held
//                still before it lifts so nothing flings
//   pinch FACTOR [X Y]  two fingers about X Y (the centre if left out)
//                spread apart (or together) to scale by FACTOR, held still
//                before they lift
//   quit         ends it
// Each command done is logged in MARKS_DIR/log with the wall-clock time.
//
//   scripts/ios.sh --dev --device <udid> \
//     --target integration_test/phone_camera_test.dart \
//     --define MARKS_DIR=/tmp/phone-camera
//   xcrun simctl install <udid> app/build/ios/iphonesimulator/Runner.app
//   xcrun simctl launch <udid> com.claktory.app.dev
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/inbox.dart' show InboxView;
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/mobile/needs_you_button.dart'
    show PhoneNeedsYouButton;
import 'package:factory_app/mobile/composer.dart' show SendButton;
import 'package:factory_app/mobile/voice_input.dart' show PhoneMicButton;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _marks = String.fromEnvironment(
  'MARKS_DIR',
  defaultValue: '/tmp/phone-camera',
);

/// A file in place of the microphone: what [start] records is [file], and
/// its loudness comes from [levels], one every 50 ms in real time, as the
/// native recorder's does, for as long as it runs (demo_capture_test.dart's
/// on the Mac).
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
/// extension (nadia.m4a, nadia.levels).
File _levelsOf(String file) {
  final beside = File('$file.levels');
  if (beside.existsSync()) return beside;
  final dot = file.lastIndexOf('.');
  return File(
    '${dot > file.lastIndexOf('/') ? file.substring(0, dot) : file}.levels',
  );
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('phone camera', timeout: Timeout.none, (tester) async {
    // Menu, Back and Close menu are found by what VoiceOver hears.
    final semantics = tester.ensureSemantics();
    addTearDown(semantics.dispose);
    final control = File('$_marks/control');
    control.parent.createSync(recursive: true);
    if (!control.existsSync()) control.writeAsStringSync('');
    final log = File('$_marks/log');
    void say(String text) => log.writeAsStringSync(
      '${DateTime.now().toIso8601String()} $text\n',
      mode: FileMode.append,
    );
    // Before the app: each message box takes its recorder as it's made.
    final mic = _FileMic();
    deviceRecorder = mic;

    /// Lets [ms] of real time pass, drawing every frame.
    Future<void> hold([int ms = 100]) async {
      final end = DateTime.now().add(Duration(milliseconds: ms));
      while (DateTime.now().isBefore(end)) {
        await tester.pump(const Duration(milliseconds: 16));
        await Future<void>.delayed(const Duration(milliseconds: 16));
      }
    }

    bool shows(Finder finder) => finder.evaluate().isNotEmpty;
    final menu = find.bySemanticsLabel('Menu');
    final drawer = find.byType(Sidebar);

    await app.main();
    await hold(2000);

    // Not paired yet: its code pasted in, then the four digits while it's
    // allowed there.
    if (!shows(drawer) && !shows(menu)) {
      final code = File('$_marks/code');
      if (!code.existsSync()) fail('not paired, and no $_marks/code');
      for (var i = 0; i < 25; i++) {
        if (shows(find.byType(EditableText))) break;
        if (shows(find.text('Paste code instead'))) {
          await tester.tap(find.text('Paste code instead'));
        }
        await hold(300);
      }
      await hold(800);
      await tester.enterText(
        find.byType(EditableText).first,
        code.readAsStringSync().trim(),
      );
      await hold(1200);
      await tester.tap(find.text('Connect').last);
      // Then "Connect to this Claktory?", its code to check: Connect again.
      for (var i = 0; i < 50; i++) {
        if (shows(find.text('Connect to this Claktory?'))) break;
        await hold(200);
      }
      if (shows(find.text('Connect to this Claktory?'))) {
        await hold(1200);
        await tester.tap(find.text('Connect').last);
      }
      say('pairing: allow this device on the Claktory');
    }
    final began = DateTime.now();
    while (!shows(find.byType(Shell)) ||
        !AppScope.of(tester.element(find.byType(Shell).first))
            .connected
            .value) {
      if (DateTime.now().difference(began) > const Duration(minutes: 5)) {
        fail('never connected');
      }
      await hold(200);
    }
    Workspace workspace() =>
        AppScope.of(tester.element(find.byType(Shell).first))
            .workspace
            .workspace;
    await hold(1500);
    File('$_marks/ready').writeAsStringSync(DateTime.now().toIso8601String());
    say('ready: append commands to ${control.path}');

    Future<void> openDrawer() async {
      if (shows(drawer)) return;
      await tester.tap(menu.first);
      await hold(700);
    }

    Future<void> tapInDrawer(String text) async {
      final row = find.descendant(of: drawer, matching: find.text(text));
      if (!shows(row)) {
        say('not in the menu: $text');
        return;
      }
      await tester.ensureVisible(row.first);
      await hold(300);
      await tester.tap(row.first);
      await hold(900);
    }

    /// A point given as "X Y" in screen points, or the screen's centre.
    Offset? pointOf(Iterable<String> words) {
      final screen = tester.view.physicalSize / tester.view.devicePixelRatio;
      if (words.isEmpty) return Offset(screen.width / 2, screen.height / 2);
      final xy = words.map(double.tryParse).toList();
      if (xy.length != 2 || xy.contains(null)) return null;
      return Offset(xy[0]!, xy[1]!);
    }

    /// Fingers down at [from], moved together to [to] over [ms] in small
    /// steps, eased out, then held still before they lift, so nothing
    /// flings.
    Future<void> moveFingers(List<Offset> from, List<Offset> to, int ms) async {
      final fingers = [
        for (final (i, point) in from.indexed)
          await tester.startGesture(point, pointer: 900 + i),
      ];
      final steps = (ms / 32).ceil().clamp(6, 60);
      var was = from;
      for (var step = 1; step <= steps; step++) {
        final t = 1 - (1 - step / steps) * (1 - step / steps);
        final now = [
          for (final (i, point) in from.indexed) Offset.lerp(point, to[i], t)!,
        ];
        for (final (i, finger) in fingers.indexed) {
          await finger.moveBy(now[i] - was[i]);
        }
        was = now;
        await hold(32);
      }
      await hold(400);
      for (final finger in fingers) {
        await finger.up();
      }
      await hold(900);
    }

    var read = control.lengthSync();
    while (true) {
      await hold(100);
      final length = control.lengthSync();
      if (length <= read) continue;
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
        final arg = space < 0 ? '' : command.substring(space + 1).trim();
        say(command);
        switch (verb) {
          case 'quit':
            return;
          case 'hold':
            break;
          case 'project':
            final project = [
              for (final project in workspace().projects)
                if (project.id == arg || project.name == arg) project,
            ].firstOrNull;
            if (project == null) {
              say('no project $arg');
              break;
            }
            await openDrawer();
            // From All projects, unless it's the one shown already.
            final all = find.descendant(
              of: drawer,
              matching: find.text('‹ All projects'),
            );
            if (workspace().focusedProjectId != project.id && shows(all)) {
              await tapInDrawer('‹ All projects');
            }
            if (workspace().focusedProjectId != project.id) {
              await tapInDrawer(project.name);
            }
            // At the top of its list, nothing scrolled.
            final list = find.descendant(
              of: drawer,
              matching: find.byType(Scrollable),
            );
            if (shows(list)) {
              final state = tester.state<ScrollableState>(list.first);
              state.position.jumpTo(state.position.minScrollExtent);
            }
            await hold(600);
          case 'select':
            final agent = [
              for (final agent in workspace().agents)
                if (agent.id == arg || agent.name == arg) agent,
            ].firstOrNull;
            if (agent == null) {
              say('no agent $arg');
              break;
            }
            await openDrawer();
            final row = find.descendant(
              of: drawer,
              matching: find.text(agent.name),
            );
            if (shows(row)) {
              await tapInDrawer(agent.name);
            } else {
              // In a project not shown: as the search would find it.
              await AppScope.of(tester.element(find.byType(Shell).first))
                  .selectAgent(agent.id);
            }
            for (var i = 0; i < 50; i++) {
              if (workspace().selectedAgentId == agent.id && !shows(drawer)) {
                break;
              }
              await hold(100);
            }
          case 'voice':
            mic
              ..file = arg
              ..levels = [
                for (final line in _levelsOf(arg).readAsLinesSync())
                  if (line.trim().isNotEmpty) double.parse(line),
              ]
              ..played = Completer<void>();
            var played = false;
            unawaited(mic.played.future.then((_) => played = true));
            final record = find.byType(PhoneMicButton);
            if (!shows(record)) {
              say('no mic: open a conversation first');
              break;
            }
            await tester.tap(record.last);
            final until = DateTime.now().add(
              const Duration(milliseconds: 50) * mic.levels.length +
                  const Duration(seconds: 30),
            );
            while (!played && DateTime.now().isBefore(until)) {
              await hold(50);
            }
            await hold(300);
            final send = find.byType(SendButton);
            if (!shows(send)) {
              say("voice didn't record: no Send");
              break;
            }
            await tester.tap(send.last);
            await hold(300);
            say('voice sent');
          case 'inbox':
            final header = find.byType(PhoneNeedsYouButton);
            final row = find.byKey(const ValueKey('needs-you'));
            if (shows(header) && !shows(drawer)) {
              await tester.tap(header.first);
            } else {
              await openDrawer();
              if (!shows(row)) {
                say('no Needs you: nothing needs you');
                break;
              }
              await tester.tap(row.first);
            }
            await hold(1200);
          case 'open':
            final card = find.descendant(
              of: find.byType(InboxView),
              matching: find.textContaining(arg),
            );
            if (!shows(card)) {
              say('not found: $arg');
              break;
            }
            await tester.ensureVisible(card.first);
            await hold(300);
            await tester.tap(card.first);
            await hold(1200);
          case 'tap':
            final target = find.textContaining(arg, findRichText: true);
            if (!shows(target)) {
              say('not found: $arg');
              break;
            }
            await tester.ensureVisible(target.last);
            await hold(600);
            await tester.tap(target.last);
            await hold(1200);
            say('tapped: $arg');
          case 'swipe':
            final words = arg.split(RegExp(r'\s+'));
            final points = double.tryParse(words.first);
            final at = pointOf(words.skip(1));
            if (points == null || at == null) {
              say('swipe: not N [X Y]: $arg');
              break;
            }
            await moveFingers(
              [at],
              [at - Offset(0, points)],
              (points.abs() * 2).clamp(200, 1200).round(),
            );
          case 'pinch':
            final words = arg.split(RegExp(r'\s+'));
            final factor = double.tryParse(words.first);
            final at = pointOf(words.skip(1));
            if (factor == null || factor <= 0 || at == null) {
              say('pinch: not FACTOR [X Y]: $arg');
              break;
            }
            // Two fingers 120 points apart, across, moved apart (or
            // together) to FACTOR times that.
            const half = Offset(60, 0);
            await moveFingers(
              [at - half, at + half],
              [at - half * factor, at + half * factor],
              (factor - 1).abs().clamp(0.25, 2) * 600 ~/ 1,
            );
          case 'scroll':
            // Claktory's own search of the whole conversation, newest
            // first, a page at a time: the last match is the oldest.
            final backend = AppScope.of(
              tester.element(find.byType(Shell).first),
            );
            final agentId = workspace().selectedAgentId;
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
              say('not found: $arg (${error.message})');
              break;
            }
            if (first == null) {
              say('not found: $arg');
              break;
            }
            backend.reveal.reachSeq(agentId, first.seq);
            // Until the conversation has gone there (loading back to it
            // first), and laid out there.
            for (var i = 0; i < 200 && backend.reveal.value != null; i++) {
              await hold(50);
            }
            await hold(400);
            say('scrolled: ${first.snippet}');
          case 'back':
            final back = find.bySemanticsLabel('Back');
            if (shows(drawer)) {
              // The strip of dimmed conversation beside it, as a thumb
              // closes it.
              final screen =
                  tester.view.physicalSize / tester.view.devicePixelRatio;
              await tester.tapAt(Offset(screen.width - 8, screen.height / 2));
            } else if (shows(back)) {
              await tester.tap(back.last);
            }
            await hold(600);
          default:
            say('unknown command: $command');
        }
        say('done: $command');
      }
    }
  });
}
