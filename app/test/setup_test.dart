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
}

void main() {
  group('the first launch sets this Mac up', () {
    late ValueNotifier<bool> connected;
    late int reconnects;

    MacSetup setupWith(
      FakeSetupChannel channel, {
      Duration startWithin = const Duration(seconds: 30),
    }) {
      connected = ValueNotifier(false);
      reconnects = 0;
      return MacSetup(
        channel: channel,
        connected: connected,
        reconnect: () => reconnects++,
        startWithin: startWithin,
      );
    }

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
      await Future<void>.delayed(const Duration(milliseconds: 100));
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
