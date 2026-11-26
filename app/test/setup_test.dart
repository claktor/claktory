import 'dart:async';

import 'package:factory_app/scope.dart';
import 'package:factory_app/setup.dart';
import 'package:factory_app/shell.dart';
import 'package:factory_app/update.dart' show AppBuild;
import 'package:factory_app/zoom.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';

/// Answers as Setup.swift and the bundled `factoryd install` would, from
/// what the test sets, and records what was run.
class FakeSetupChannel implements SetupChannel {
  FakeSetupChannel({
    this.needsMove = false,
    this.fromImage = true,
    this.helpers = true,
    this.installed = false,
    Map<String, InstallResult>? results,
  }) : results = results ?? {};

  bool needsMove;
  bool fromImage;
  bool helpers;
  bool installed;

  /// By the arguments joined with spaces ("install --check").
  final Map<String, InstallResult> results;

  /// Holds `install` until completed, to see what shows meanwhile.
  Completer<void>? hold;

  final runs = <String>[];
  var moves = 0;
  String? moveError;

  @override
  Future<SetupLocation> location() async => SetupLocation(
    path: needsMove ? '/Volumes/Claktory/Claktory.app' : '/Applications/C.app',
    needsMove: needsMove,
    fromImage: fromImage,
    helpers: helpers,
  );

  @override
  Future<InstallResult> install(List<String> arguments) async {
    final key = arguments.join(' ');
    runs.add(key);
    if (key == 'install') await hold?.future;
    return results[key] ?? const InstallResult(result: 'failed');
  }

  @override
  Future<InstallResult> uninstall(List<String> arguments) async {
    final key = arguments.join(' ');
    runs.add(key);
    return results[key] ?? const InstallResult(result: 'failed');
  }

  /// Each removeApp's forget.
  final removed = <bool>[];

  @override
  Future<void> removeApp({required bool forget}) async => removed.add(forget);

  @override
  Future<String?> moveToApplications() async {
    moves++;
    return moveError;
  }

  @override
  bool installedHere() => installed;

  /// The ClaktoryAppOnly user default.
  bool appOnlyMark = false;

  @override
  Future<bool> appOnly() async => appOnlyMark;

  @override
  Future<void> setAppOnly(bool appOnly) async => appOnlyMark = appOnly;

  /// What macOS says of the LaunchAgent.
  BackgroundStatus background = BackgroundStatus.enabled;

  /// How many times it was asked: each look once out of reach.
  var backgroundAsks = 0;

  @override
  Future<BackgroundStatus> backgroundStatus() async {
    backgroundAsks++;
    return background;
  }

  var loginItemsOpened = 0;

  @override
  Future<void> openLoginItems() async => loginItemsOpened++;
}

/// Waits until [ok], on the real clock: these tests run [MacSetup]'s own
/// timers, so a loaded machine (a bundle of test files sharing a process,
/// two runs at once) may fire them late. A fixed wait failed there.
Future<void> until(
  bool Function() ok, {
  Duration within = const Duration(seconds: 3),
}) async {
  final end = DateTime.now().add(within);
  while (!ok()) {
    if (DateTime.now().isAfter(end)) return;
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
}

/// Waits until [channel] ran `install --check` [times] times, and what
/// came of the last has been taken in.
Future<void> checked(FakeSetupChannel channel, int times) async {
  await until(
    () => channel.runs.where((run) => run == 'install --check').length >= times,
  );
  await pumpEventQueue();
}

void main() {
  group('the first launch sets this Mac up', () {
    late ValueNotifier<bool> connected;
    late int reconnects;

    MacSetup setupWith(
      FakeSetupChannel channel, {
      Duration startWithin = const Duration(seconds: 30),
      Duration lostFor = const Duration(seconds: 5),
      Duration lookEvery = const Duration(seconds: 2),
    }) {
      connected = ValueNotifier(false);
      reconnects = 0;
      return MacSetup(
        channel: channel,
        connected: connected,
        reconnect: () => reconnects++,
        startWithin: startWithin,
        lostFor: lostFor,
        lookEvery: lookEvery,
      );
    }

    group('stopped and started (friend bug #27)', () {
      test('stopped by the app quitting: opening it starts it', () async {
        final channel = FakeSetupChannel(
          results: {
            'install --check': const InstallResult(result: 'stopped'),
            'install --start': const InstallResult(result: 'started'),
          },
        );
        final setup = setupWith(channel);
        addTearDown(setup.dispose);
        await setup.start();
        await pumpEventQueue();
        expect(channel.runs, ['install --check', 'install --start']);
        expect(setup.stage, SetupStage.done);
        expect(reconnects, 1);
      });

      test('turned off by the person: it waits for Start', () async {
        final channel = FakeSetupChannel(
          results: {
            'install --check': const InstallResult(
              result: 'stopped',
              held: true,
            ),
            'install --start': const InstallResult(result: 'started'),
          },
        );
        final setup = setupWith(channel);
        addTearDown(setup.dispose);
        await setup.start();
        await pumpEventQueue();
        expect(channel.runs, ['install --check']);
        expect(setup.stage, SetupStage.stopped);
        expect(setup.showing, isTrue);

        final starting = setup.startEngine();
        expect(setup.stage, SetupStage.starting);
        await starting;
        expect(channel.runs.last, 'install --start');
        expect(setup.stage, SetupStage.done);
        expect(reconnects, 1);
      });

      test('turned off while the app is open: once out of reach a moment, '
          'it says so; out of reach otherwise, it waits', () async {
        final channel = FakeSetupChannel(
          results: {
            'install --check': const InstallResult(
              result: 'already_installed',
              running: true,
            ),
          },
        );
        final setup = setupWith(
          channel,
          lostFor: const Duration(milliseconds: 20),
        );
        addTearDown(setup.dispose);
        connected.value = true;
        await setup.start();

        // Restarting (an update, say): --check says not running; it waits.
        channel.results['install --check'] = const InstallResult(
          result: 'not_running',
        );
        connected.value = false;
        // Once out of reach a moment, it looked, and waits.
        await checked(channel, 2);
        expect(setup.stage, SetupStage.done);
        connected.value = true;

        channel.results['install --check'] = const InstallResult(
          result: 'stopped',
          held: true,
        );
        connected.value = false;
        await until(() => setup.stage == SetupStage.stopped);
        expect(setup.stage, SetupStage.stopped);
      });

      test('asked to stop: once gone, turned off at once, not after a '
          'while', () async {
        final channel = FakeSetupChannel(
          results: {
            'install --check': const InstallResult(
              result: 'already_installed',
              running: true,
            ),
          },
        );
        final setup = setupWith(channel);
        addTearDown(setup.dispose);
        connected.value = true;
        await setup.start();
        setup.expectStop();
        // launchd still stopping it: not yet, then stopped.
        channel.results['install --check'] = const InstallResult(
          result: 'not_running',
        );
        connected.value = false;
        await pumpEventQueue();
        expect(setup.stage, SetupStage.done);
        channel.results['install --check'] = const InstallResult(
          result: 'stopped',
          held: true,
        );
        // At its next look, well within the 5 s a lost connection waits.
        await until(() => setup.stage == SetupStage.stopped);
        expect(setup.stage, SetupStage.stopped);
      });

      test("the result's held is read", () {
        final held = InstallResult.parse(
          4,
          '{"result":"stopped","reason":"","detail":"","version":"v","held":true}',
          '',
        );
        expect(held.result, 'stopped');
        expect(held.held, isTrue);
        expect(
          InstallResult.parse(
            0,
            '{"result":"stopped","reason":"","detail":"","version":"v"}',
            '',
          ).held,
          isFalse,
        );
      });
    });

    group('macOS blocking it in the background (friend bug #28)', () {
      test('installed and blocked: it says so at once, not Repair, and '
          'starts once allowed', () async {
        final channel = FakeSetupChannel(
          results: {
            'install --check': const InstallResult(result: 'already_installed'),
            'install --start': const InstallResult(result: 'started'),
          },
        )..background = BackgroundStatus.requiresApproval;
        final setup = setupWith(
          channel,
          lookEvery: const Duration(milliseconds: 20),
        );
        addTearDown(setup.dispose);
        await setup.start();
        await pumpEventQueue();
        expect(setup.stage, SetupStage.blocked);
        expect(setup.showing, isTrue);
        expect(channel.runs, ['install --check']);

        await setup.openLoginItems();
        expect(channel.loginItemsOpened, 1);
        // Still off: it looks again, and waits.
        final asked = channel.backgroundAsks;
        await until(() => channel.backgroundAsks >= asked + 2);
        await pumpEventQueue();
        expect(setup.stage, SetupStage.blocked);
        expect(channel.runs, ['install --check']);

        channel.background = BackgroundStatus.enabled;
        await until(() => setup.stage == SetupStage.done);
        expect(channel.runs.last, 'install --start');
        expect(setup.stage, SetupStage.done);
        expect(reconnects, 1);
      });

      test('a start launchd refuses while macOS blocks it: blocked', () async {
        final channel = FakeSetupChannel(
          results: {
            'install --check': const InstallResult(result: 'stopped'),
            'install --start': const InstallResult(
              result: 'failed',
              detail: 'Bootstrap failed: 5: Input/output error',
            ),
          },
        )..background = BackgroundStatus.requiresApproval;
        final setup = setupWith(channel);
        addTearDown(setup.dispose);
        await setup.start();
        await pumpEventQueue();
        expect(channel.runs, ['install --check', 'install --start']);
        expect(setup.stage, SetupStage.blocked);
      });

      test('not running and blocked: blocked, not Repair', () async {
        final channel = FakeSetupChannel(
          results: {
            'install --check': const InstallResult(result: 'not_running'),
          },
        )..background = BackgroundStatus.requiresApproval;
        final setup = setupWith(channel);
        addTearDown(setup.dispose);
        await setup.start();
        await pumpEventQueue();
        expect(setup.stage, SetupStage.blocked);
      });

      test('not running and allowed: Repair, as before', () async {
        final channel = FakeSetupChannel(
          results: {
            'install --check': const InstallResult(result: 'not_running'),
          },
        );
        final setup = setupWith(channel);
        addTearDown(setup.dispose);
        await setup.start();
        await pumpEventQueue();
        expect(setup.stage, SetupStage.notRunning);
      });

      test('turned off while it ran: once it has been out of reach a '
          'moment, it says so; back, it goes', () async {
        final channel = FakeSetupChannel(
          results: {
            'install --check': const InstallResult(
              result: 'already_installed',
              running: true,
            ),
          },
        );
        final setup = setupWith(
          channel,
          lostFor: const Duration(milliseconds: 20),
          lookEvery: const Duration(seconds: 10),
        );
        addTearDown(setup.dispose);
        connected.value = true;
        await setup.start();
        expect(setup.stage, SetupStage.done);

        // A moment's drop with macOS allowing it: it looks, and nothing.
        connected.value = false;
        await checked(channel, 2);
        expect(setup.stage, SetupStage.done);
        connected.value = true;

        channel.background = BackgroundStatus.requiresApproval;
        connected.value = false;
        await until(() => setup.stage == SetupStage.blocked);
        expect(setup.stage, SetupStage.blocked);
        expect(setup.showing, isTrue);

        connected.value = true;
        expect(setup.stage, SetupStage.done);
        expect(setup.showing, isFalse);
      });
    });

    test('a Mac without Claktory gets it installed, then connects', () async {
      final channel = FakeSetupChannel(
        results: {
          'install --check': const InstallResult(result: 'not_installed'),
          'install': const InstallResult(result: 'installed'),
        },
      );
      final setup = setupWith(channel);
      await setup.start();
      expect(channel.runs, ['install --check', 'install']);
      expect(setup.stage, SetupStage.done);
      expect(reconnects, 1);
    });

    test('with somewhere else to go, a Mac without Claktory is asked '
        'first: here, or a Claktory on another computer', () async {
      final channel = FakeSetupChannel(
        results: {
          'install --check': const InstallResult(result: 'not_installed'),
          'install': const InstallResult(result: 'installed'),
        },
      );
      var elsewhere = 0;
      connected = ValueNotifier(false);
      final setup = MacSetup(
        channel: channel,
        connected: connected,
        reconnect: () {},
        elsewhere: () => elsewhere++,
      );
      await setup.start();
      expect(setup.stage, SetupStage.choosing);
      expect(setup.showing, isTrue);
      expect(channel.runs, ['install --check']);
      setup.useElsewhere();
      expect(setup.stage, SetupStage.done);
      expect(setup.showing, isFalse);
      expect(elsewhere, 1);
      expect(channel.runs, ['install --check'], reason: 'nothing installed');

      // Chosen here: it's installed.
      final here = MacSetup(
        channel: channel,
        connected: connected,
        reconnect: () {},
        elsewhere: () => elsewhere++,
      );
      await here.start();
      await here.setUpHere();
      expect(channel.runs.last, 'install');
      expect(here.stage, SetupStage.done);

      // Already chosen (this Mac after all): not asked again.
      channel.runs.clear();
      final chosen = MacSetup(
        channel: channel,
        connected: connected,
        reconnect: () {},
        elsewhere: () => elsewhere++,
        ask: false,
      );
      await chosen.start();
      expect(channel.runs, ['install --check', 'install']);
    });

    test('one already installed is left alone', () async {
      final channel = FakeSetupChannel(
        results: {
          'install --check': const InstallResult(
            result: 'already_installed',
            running: true,
          ),
        },
      );
      final setup = setupWith(channel);
      await setup.start();
      expect(channel.runs, ['install --check']);
      expect(setup.stage, SetupStage.done);
      expect(setup.showing, isFalse);
    });

    test('installed but not answering in time offers Repair', () async {
      final channel = FakeSetupChannel(
        results: {
          'install --check': const InstallResult(result: 'already_installed'),
          'install --repair': const InstallResult(result: 'installed'),
        },
      );
      final setup = setupWith(
        channel,
        startWithin: const Duration(milliseconds: 50),
      );
      await setup.start();
      expect(setup.stage, SetupStage.done);
      await until(() => setup.stage == SetupStage.notRunning);
      expect(setup.stage, SetupStage.notRunning);
      expect(setup.showing, isTrue);
      expect(setup.repairable, isTrue);

      await setup.repair();
      expect(channel.runs.last, 'install --repair');
      expect(setup.stage, SetupStage.done);
      expect(reconnects, 1);
    });

    test('connecting in time offers nothing', () async {
      final channel = FakeSetupChannel(
        results: {
          'install --check': const InstallResult(result: 'already_installed'),
        },
      );
      final setup = setupWith(
        channel,
        startWithin: const Duration(milliseconds: 50),
      );
      await setup.start();
      connected.value = true;
      await Future<void>.delayed(const Duration(milliseconds: 100));
      expect(setup.stage, SetupStage.done);
    });

    test('an incomplete install offers Repair, a failure Try again', () async {
      final channel = FakeSetupChannel(
        results: {
          'install --check': const InstallResult(
            result: 'incomplete',
            reason: 'Part of Claktory is missing on this Mac.',
            detail: 'no ~/.factory/bin/claktory-apps',
          ),
          'install --repair': const InstallResult(
            result: 'failed',
            reason: "There isn't enough room on this Mac.",
          ),
        },
      );
      final setup = setupWith(channel);
      await setup.start();
      expect(setup.stage, SetupStage.failed);
      expect(setup.repairable, isTrue);
      expect(setup.reason, 'Part of Claktory is missing on this Mac.');
      await setup.repair();
      expect(setup.stage, SetupStage.failed);
      expect(setup.repairable, isFalse);
      expect(setup.reason, "There isn't enough room on this Mac.");
      // Connected after all: nothing over the window.
      connected.value = true;
      expect(setup.stage, SetupStage.done);
      expect(setup.showing, isFalse);
    });

    test('a damaged copy is downloaded again, not tried again', () async {
      final setup = setupWith(
        FakeSetupChannel(
          results: {
            'install --check': const InstallResult(result: 'not_installed'),
            'install': const InstallResult(
              result: 'damaged',
              reason:
                  'This copy of Claktory is damaged. Download Claktory again.',
            ),
          },
        ),
      );
      await setup.start();
      expect(setup.stage, SetupStage.failed);
      expect(setup.download, isTrue);
      expect(setup.repairable, isFalse);
    });

    test('without helpers: fine when installed, else download again', () async {
      final installed = setupWith(
        FakeSetupChannel(helpers: false, installed: true),
      );
      await installed.start();
      expect(installed.stage, SetupStage.done);
      final missing = setupWith(FakeSetupChannel(helpers: false));
      await missing.start();
      expect(missing.stage, SetupStage.missing);
    });

    test('from the disk image it offers to move first', () async {
      final channel = FakeSetupChannel(
        needsMove: true,
        results: {
          'install --check': const InstallResult(
            result: 'already_installed',
            running: true,
          ),
        },
      );
      final setup = setupWith(channel);
      await setup.start();
      expect(setup.stage, SetupStage.offerMove);
      expect(channel.runs, isEmpty);
      // Connected or not, the offer stays.
      connected.value = true;
      expect(setup.showing, isTrue);

      channel.moveError = 'Not enough room.';
      await setup.moveToApplications();
      expect(channel.moves, 1);
      expect(setup.stage, SetupStage.offerMove);
      expect(setup.moveError, 'Not enough room.');

      await setup.stayHere();
      expect(channel.runs, ['install --check']);
      expect(setup.stage, SetupStage.done);
    });

    test("factoryd's line is read; anything else is a failure", () {
      final read = InstallResult.parse(
        4,
        'progress\n{"result":"already_installed","reason":"",'
            '"detail":"","version":"v0.1.10","running":true}\n',
        '',
      );
      expect(read.result, 'already_installed');
      expect(read.running, isTrue);
      expect(read.version, 'v0.1.10');
      final broken = InstallResult.parse(2, 'usage: …', 'bad flag');
      expect(broken.result, 'failed');
      expect(broken.detail, contains('bad flag'));
    });
  });

  group('the step over the window', () {
    Future<(Harness, FakeSetupChannel, MacSetup)> show(
      WidgetTester tester,
      FakeSetupChannel channel,
    ) async {
      final harness = Harness(workspace());
      harness.backend.connected.value = false;
      final setup = MacSetup(
        channel: channel,
        connected: harness.backend.connected,
        reconnect: harness.backend.reconnectNow,
      );
      addTearDown(setup.dispose);
      tester.view.physicalSize = const Size(1200, 900);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      await tester.pumpWidget(
        WidgetsApp(
          color: const Color(0xFF000000),
          builder: (context, _) => SetupScope(
            setup: setup,
            child: AppScope(
              backend: harness.backend,
              login: harness.login,
              page: harness.page,
              appBuild: AppBuild(
                version: 'v0.1.10',
                bundlePath: '/x',
                quit: () {},
              ),
              drafts: harness.drafts,
              history: harness.history,
              connections: harness.connections,
              camera: harness.camera,
              child: const AppZoom(child: Shell()),
            ),
          ),
        ),
      );
      unawaited(setup.start());
      await tester.pump();
      await tester.pump();
      return (harness, channel, setup);
    }

    testWidgets('setting up, then a failure with its reason and details', (
      tester,
    ) async {
      final channel = FakeSetupChannel(
        results: {
          'install --check': const InstallResult(result: 'not_installed'),
          'install': const InstallResult(
            result: 'failed',
            reason: "Claktory's helper isn't signed correctly.",
            detail: 'codesign: invalid signature',
          ),
        },
      )..hold = Completer();
      await show(tester, channel);
      expect(find.text('Setting up Claktory…'), findsOneWidget);
      expect(find.text('This takes a few seconds.'), findsOneWidget);

      channel.hold!.complete();
      await tester.pump();
      await tester.pump();
      expect(find.text("Claktory couldn't finish setting up"), findsOneWidget);
      expect(
        find.text("Claktory's helper isn't signed correctly."),
        findsOneWidget,
      );
      expect(find.text('codesign: invalid signature'), findsNothing);
      await tester.tap(find.text('Show details'));
      await tester.pump();
      expect(find.text('codesign: invalid signature'), findsOneWidget);

      channel.results['install'] = const InstallResult(result: 'installed');
      await tester.tap(find.text('Try again'));
      await tester.pump();
      await tester.pump();
      expect(find.text("Claktory couldn't finish setting up"), findsNothing);
      expect(channel.runs.last, 'install');
    });

    testWidgets('blocked by macOS: what to do, and Open System Settings', (
      tester,
    ) async {
      final channel = FakeSetupChannel(
        results: {
          'install --check': const InstallResult(result: 'not_running'),
        },
      )..background = BackgroundStatus.requiresApproval;
      await show(tester, channel);
      await tester.pump();
      expect(
        find.text('macOS is blocking Claktory from running in the background'),
        findsOneWidget,
      );
      expect(find.textContaining('Allow in the Background'), findsOneWidget);
      expect(find.text('Repair'), findsNothing);
      expect(find.text('Not now'), findsNothing);
      await tester.tap(find.text('Open System Settings'));
      await tester.pump();
      expect(channel.loginItemsOpened, 1);
    });

    testWidgets('turned off: says so, and Start Claktory starts it', (
      tester,
    ) async {
      final channel = FakeSetupChannel(
        results: {
          'install --check': const InstallResult(result: 'stopped', held: true),
          'install --start': const InstallResult(result: 'started'),
        },
      );
      await show(tester, channel);
      await tester.pump();
      expect(find.text('Claktory is turned off'), findsOneWidget);
      await tester.tap(find.text('Start Claktory'));
      await tester.pump();
      await tester.pump();
      expect(channel.runs.last, 'install --start');
    });

    testWidgets('Move to Applications, or Not now', (tester) async {
      final channel = FakeSetupChannel(
        needsMove: true,
        results: {
          'install --check': const InstallResult(
            result: 'already_installed',
            running: true,
          ),
        },
      );
      await show(tester, channel);
      expect(find.text('Move Claktory to Applications?'), findsOneWidget);
      expect(
        find.textContaining('Claktory is running from the disk image.'),
        findsOneWidget,
      );
      // It couldn't (moved, the app would have quit).
      channel.moveError = "You don't have permission.";
      await tester.tap(find.text('Move to Applications'));
      await tester.pump();
      await tester.pump();
      expect(channel.moves, 1);
      expect(find.text("You don't have permission."), findsOneWidget);

      await tester.tap(find.text('Not now'));
      await tester.pump();
      await tester.pump();
      expect(find.text('Move Claktory to Applications?'), findsNothing);
      expect(channel.runs, ['install --check']);
    });

    testWidgets('a temporary copy macOS made is called that', (tester) async {
      await show(tester, FakeSetupChannel(needsMove: true, fromImage: false));
      expect(
        find.textContaining('macOS is running a temporary copy of Claktory.'),
        findsOneWidget,
      );
      expect(find.textContaining('disk image'), findsNothing);
    });

    testWidgets('a damaged copy offers Download, not Try again', (
      tester,
    ) async {
      await show(
        tester,
        FakeSetupChannel(
          results: {
            'install --check': const InstallResult(result: 'not_installed'),
            'install': const InstallResult(
              result: 'damaged',
              reason:
                  'This copy of Claktory is damaged. Download Claktory again.',
            ),
          },
        ),
      );
      await tester.pump();
      expect(find.text('Download'), findsOneWidget);
      expect(find.text('Try again'), findsNothing);
    });

    testWidgets('without helpers or an install: download again', (
      tester,
    ) async {
      await show(tester, FakeSetupChannel(helpers: false));
      expect(find.text('Download Claktory again'), findsOneWidget);
      expect(find.text('Download'), findsOneWidget);
    });
  });
}
