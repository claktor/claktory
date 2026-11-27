import 'package:factory_app/connections.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/tunnel_client.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace focusedOn(
  String projectId, {
  String version = '',
  String latest = '',
  String latestBackend = '',
  String updateError = '',
  bool working = false,
}) => Workspace(
  projects: [
    Project(id: 'p1', name: 'Factory', path: '/Users/me/factory'),
    Project(id: 'p2', name: 'Site', path: '/Users/me/.factory/workspace/site'),
  ],
  agents: [
    Agent(
      id: 'rivet',
      name: 'Rivet',
      modelAvailable: true,
      status: working
          ? AgentStatus.AGENT_STATUS_WORKING
          : AgentStatus.AGENT_STATUS_IDLE,
    ),
    Agent(id: 'a1', projectId: 'p1', name: 'Builder', modelAvailable: true),
    Agent(id: 'a2', projectId: 'p2', name: 'Writer', modelAvailable: true),
  ],
  selectedAgentId: projectId == 'p1' ? 'a1' : '',
  focusedProjectId: projectId,
  version: version,
  latestRelease: latest,
  latestBackend: latestBackend,
  updateError: updateError,
);

Explorer tree({
  String projectId = 'p1',
  bool showAll = false,
  List<FileEntry>? entries,
  String error = '',
}) => Explorer(
  projectId: projectId,
  showAll: showAll,
  error: error,
  entries:
      entries ??
      [
        FileEntry(path: 'app', name: 'app', isDir: true, expanded: true),
        FileEntry(path: 'app/lib', name: 'lib', depth: 1, isDir: true),
        FileEntry(path: 'app/pubspec.yaml', name: 'pubspec.yaml', depth: 1),
        FileEntry(path: 'crates', name: 'crates', isDir: true),
        FileEntry(path: 'README.md', name: 'README.md'),
      ],
);

Future<Harness> showFocused(
  WidgetTester tester,
  Workspace snapshot, {
  Explorer? explorer,
}) async {
  final harness = Harness(snapshot);
  if (explorer != null) harness.backend.explorer.apply(explorer);
  await harness.show(tester);
  return harness;
}

void main() {
  testWidgets(
    "a click folds a project; its menu's Show only this project focuses "
    'it, and focused, the sidebar is only that project (§4)',
    (tester) async {
      final harness = await showFocused(tester, focusedOn(''));
      expect(find.text('Rivet'), findsOneWidget);
      await tester.tap(find.text('Site'));
      await settle(tester);
      expect(
        harness.last.setProjectCollapsed,
        SetProjectCollapsed(projectId: 'p2', collapsed: true),
      );
      await tester.tap(find.text('Site'), buttons: kSecondaryButton);
      await settle(tester);
      await tester.tap(find.text('Show only this project'));
      await settle(tester);
      expect(harness.last.focusProject.projectId, 'p2');

      await harness.apply(tester, focusedOn('p1'));
      await settle(tester);
      expect(find.text('‹ All projects'), findsOneWidget);
      expect(find.text('Builder'), findsWidgets);
      // This Rivet is no admin agent, so it isn't pinned.
      for (final elsewhere in ['Rivet', 'Writer', 'Site', 'Workspace']) {
        expect(find.text(elsewhere), findsNothing, reason: elsewhere);
      }
      // The Projects section's heading stays, over the focused project.
      expect(find.text('Projects'), findsOneWidget);

      // The heading opens the project's menu, as its row does, by click
      // or right click.
      await tester.tap(find.text('Factory').first, buttons: kSecondaryButton);
      await settle(tester);
      expect(find.text('Show all projects'), findsOneWidget);
      await tester.tap(find.text('Project settings…'));
      await settle(tester);
      expect(find.text('Delete project…'), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await settle(tester);
      await tester.tap(find.text('Factory').first);
      await settle(tester);
      await tester.tap(find.text('Show all projects'));
      await settle(tester);
      expect(harness.last.focusProject.projectId, '');
      await harness.apply(tester, focusedOn('p1'));
      await settle(tester);

      await tester.tap(find.text('‹ All projects'));
      await settle(tester);
      expect(harness.last.focusProject.projectId, '');
    },
  );

  testWidgets('focused, Rivet stays at the top while there is an admin agent', (
    tester,
  ) async {
    final snapshot = focusedOn('p1');
    snapshot.agents[0].extensions.add(
      AgentExtension(
        id: 'agents',
        enabled: true,
        options: {'admin': OptionValue(on: true)}.entries,
      ),
    );
    final harness = await showFocused(tester, snapshot);
    expect(find.text('Rivet'), findsOneWidget);
    expect(find.text('Writer'), findsNothing);
    expect(
      tester.getTopLeft(find.text('Rivet')).dy,
      lessThan(tester.getTopLeft(find.text('‹ All projects')).dy),
    );
    await tester.tap(find.text('Rivet'));
    await settle(tester);
    expect(harness.last.selectAgent.agentId, 'rivet');
    expect(
      harness.backend.sent.where((message) => message.hasFocusProject()),
      isEmpty,
      reason: 'the project stays focused',
    );
  });

  testWidgets('a new project opens focused', (tester) async {
    final snapshot = focusedOn('')..agents[0].modelAvailable = false;
    final harness = await showFocused(tester, snapshot);
    harness.backend.onSend = (message) {
      if (message.hasCreateProject()) {
        // The new project arrives before the ack.
        harness.backend.workspace.apply(
          snapshot.deepCopy()..projects.add(Project(id: 'docs', name: 'Docs')),
        );
      }
    };
    // Without a Rivet that can run, the "+" opens the form.
    await tester.tap(askRivetButtons());
    await settle(tester);
    await tester.enterText(find.byType(EditableText).last, 'Docs');
    await tester.pump();
    await tester.tap(find.text('Create'));
    await settle(tester);
    expect(
      [
        for (final message in harness.backend.sent)
          // Where its folder would go, asked as it's named.
          if (!message.hasPreviewProjectFolder()) message.whichBody(),
      ],
      [ClientMessage_Body.createProject, ClientMessage_Body.focusProject],
    );
    expect(harness.last.focusProject.projectId, 'docs');
  });

  // A desktop's banner, whose Update replaces this app too.
  final desktop = TargetPlatformVariant.only(TargetPlatform.macOS);
  group('updates', () {
    testWidgets('an older backend is updated in place, and the version shows', (
      tester,
    ) async {
      final harness = Harness(
        focusedOn('', version: '2026.11.18-1600', latest: '2026.11.18-1700'),
      );
      harness.appVersion = '2026.11.18-1700';
      await harness.show(tester);
      expect(find.text('Claktory 2026.11.18-1700 is ready.'), findsOneWidget);
      expect(
        find.text('Claktory 2026.11.18-1600 · app 2026.11.18-1700'),
        findsOneWidget,
      );

      await tester.tap(find.text('Update'));
      await settle(tester);
      expect(
        harness.last.installUpdate.appPath,
        '',
        reason: 'the app is already new',
      );
      expect(harness.quits, 0);
      expect(find.text('Updating…'), findsOneWidget);
    }, variant: desktop);

    testWidgets('an older app is replaced too, and quits so it can be', (
      tester,
    ) async {
      final harness = Harness(
        focusedOn('', version: '2026.11.18-1700', latest: '2026.11.18-1700'),
      );
      harness.appVersion = '2026.11.18-1600';
      await harness.show(tester);
      await tester.tap(find.text('Update'));
      await settle(tester);
      expect(harness.last.installUpdate.appPath, '/Applications/Factory.app');
      expect(harness.quits, 1);
    }, variant: desktop);

    testWidgets('a refused update says why and can be tried again', (
      tester,
    ) async {
      final harness = Harness(
        focusedOn('', version: '2026.11.18-1600', latest: '2026.11.18-1700'),
      );
      harness.backend.refuse = 'no release has been published';
      await harness.show(tester);
      await tester.tap(find.text('Update'));
      await settle(tester);
      expect(
        find.text("Couldn't update: no release has been published"),
        findsOneWidget,
      );
      expect(find.text('Try again'), findsOneWidget);
    }, variant: desktop);

    /// A Factory on another machine, its backend at [version] and its
    /// latest release [latest], shown by an app at [app].
    Future<Harness> showRemote(
      WidgetTester tester, {
      required String version,
      required String app,
      String latest = '2026.11.18-1700',
    }) async {
      final harness = Harness(Workspace());
      harness.backend.connectTo(
        parsePairingCode(
          'factory://100.64.0.2:7443?t=${'a' * 64}&fp=${'b' * 64}&name=mac-mini',
        ),
      );
      harness.backend.workspace.apply(
        focusedOn('', version: version, latest: latest)
          ..remote = RemoteAccess(on: true, machine: 'mac-mini'),
      );
      harness.appVersion = app;
      await harness.show(tester);
      return harness;
    }

    testWidgets(
      "on another machine's Factory, the Claktory updates first, then an "
      "older app fetches the release's app, shows how far it is, has it "
      'swapped in and quits (#408)',
      (tester) async {
        final harness = await showRemote(
          tester,
          version: '2026.11.18-1600',
          app: '2026.11.18-1600',
        );
        expect(
          find.textContaining('Claktory 2026.11.18-1700 is ready.'),
          findsOneWidget,
        );
        await tester.tap(find.text('Update'));
        await tester.pump();
        expect(
          harness.last.installUpdate.appPath,
          '',
          reason:
              'the Claktory has the release already; the other machine '
              'has no say over this bundle',
        );
        expect(harness.backend.fetched, [('2026.11.18-1700', 0)]);
        expect(find.text('Getting the app ready on mac-mini…'), findsOneWidget);
        expect(find.text('Updating…'), findsOneWidget);
        await tester.pump(const Duration(milliseconds: 100));
        expect(
          find.text('Downloading the app from mac-mini: 12.3 of 48.0 MB'),
          findsOneWidget,
        );
        await tester.pump(const Duration(milliseconds: 100));
        expect(
          find.text(
            'Waiting for the connection to mac-mini to carry on: 12.3 of '
            '48.0 MB so far',
          ),
          findsOneWidget,
        );
        await tester.pump(const Duration(milliseconds: 100));
        expect(find.text('Checking the new app…'), findsOneWidget);
        expect(harness.quits, 0);
        await tester.pump(const Duration(milliseconds: 100));
        expect(
          harness.backend.sent.where((m) => m.hasInstallUpdate()),
          hasLength(1),
        );
        expect(harness.updater.replaced, [
          '/Applications/Factory.app.staged -> /Applications/Factory.app',
        ]);
        expect(harness.quits, 1);
      },
      variant: desktop,
    );

    testWidgets(
      "on another machine's Factory reached by a relay, the app's download "
      'is told so, to go gently (friend bug #51)',
      (tester) async {
        final harness = await showRemote(
          tester,
          version: '2026.11.18-1700',
          app: '2026.11.18-1600',
        );
        harness.backend.tunnel.value = const TunnelStatus(
          phase: TunnelPhase.connected,
          path: TunnelPath.relayed,
          error: '',
          failure: TunnelFailure.none,
          relayConnected: true,
          relayUpMs: 0,
          relayTrouble: TunnelRelayTrouble.none,
          received: 0,
          linkReceived: 0,
          dialTimeoutMs: 0,
          reach: TunnelReach.none,
        );
        await tester.tap(find.text('Update'));
        await tester.pump();
        expect(harness.updater.relayedAtStart, isTrue);
        await tester.pump(const Duration(milliseconds: 500));
      },
      variant: desktop,
    );

    testWidgets(
      'a banner made again while the update is under way shows it, and '
      'starts no second download',
      (tester) async {
        final harness = await showRemote(
          tester,
          version: '2026.11.18-1600',
          app: '2026.11.18-1600',
        );
        await tester.tap(find.text('Update'));
        await tester.pump(const Duration(milliseconds: 50));
        // The window rebuilt without the banner, then with it.
        final workspace = harness.backend.workspace.workspace;
        harness.backend.workspace.apply(
          workspace.deepCopy()..latestRelease = '',
        );
        await tester.pump();
        expect(find.text('Updating…'), findsNothing);
        harness.backend.workspace.apply(workspace);
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 100));
        expect(find.text('Updating…'), findsOneWidget);
        expect(
          find.text('Downloading the app from mac-mini: 12.3 of 48.0 MB'),
          findsOneWidget,
        );
        await tester.pump(const Duration(milliseconds: 400));
        expect(harness.backend.fetched, hasLength(1));
        expect(harness.updater.replaced, hasLength(1));
        expect(harness.quits, greaterThanOrEqualTo(1));
      },
      variant: desktop,
    );

    testWidgets(
      "on another machine's Factory, a release this app can't work with is "
      'downloaded before the Claktory updates',
      (tester) async {
        // A Mac's app, which replaces itself: a phone's can't follow.
        debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
        final harness = await showRemote(
          tester,
          version: 'v0.1.9',
          app: 'v0.1.9',
          latest: 'v0.2.0',
        );
        await tester.tap(find.text('Update'));
        await tester.pump();
        expect(harness.backend.fetched, [('v0.2.0', 0)]);
        await tester.pump(const Duration(milliseconds: 300));
        expect(find.text('Checking the new app…'), findsOneWidget);
        expect(
          harness.backend.sent.where((m) => m.hasInstallUpdate()),
          isEmpty,
          reason: 'the app it would face is not yet here',
        );
        await tester.pump(const Duration(milliseconds: 100));
        expect(harness.last.installUpdate.appPath, '');
        expect(harness.quits, 1);
        debugDefaultTargetPlatformOverride = null;
      },
      variant: desktop,
    );

    testWidgets(
      "on another machine's Factory, a failed download says why and can be "
      'tried again',
      (tester) async {
        final harness = await showRemote(
          tester,
          version: '2026.11.18-1700',
          app: '2026.11.18-1600',
        );
        harness.updater.fails = "the download is damaged";
        await tester.tap(find.text('Update'));
        await tester.pump(const Duration(milliseconds: 500));
        expect(
          find.text("Couldn't update: the download is damaged"),
          findsOneWidget,
        );
        expect(find.textContaining('Checking'), findsNothing);
        expect(harness.quits, 0);

        harness.updater.fails = null;
        await tester.tap(find.text('Try again'));
        await tester.pump(const Duration(milliseconds: 500));
        expect(harness.quits, 1);
      },
      variant: desktop,
    );

    testWidgets(
      "on another machine's Factory, a refused install fetches nothing and "
      'leaves the app as it is',
      (tester) async {
        final harness = await showRemote(
          tester,
          version: '2026.11.18-1600',
          app: '2026.11.18-1600',
        );
        harness.backend.refuse = 'no release has been published';
        await tester.tap(find.text('Update'));
        await tester.pump();
        await tester.pump();
        expect(
          find.text("Couldn't update: no release has been published"),
          findsOneWidget,
        );
        expect(harness.backend.fetched, isEmpty);
        expect(harness.updater.replaced, isEmpty);
        expect(harness.quits, 0);
      },
      variant: desktop,
    );

    testWidgets(
      "on another machine's Factory with the app up to date, only its "
      'backend updates',
      (tester) async {
        final harness = await showRemote(
          tester,
          version: '2026.11.18-1600',
          app: '2026.11.18-1700',
        );
        await tester.tap(find.text('Update'));
        await settle(tester);
        expect(harness.backend.fetched, isEmpty);
        expect(harness.last.installUpdate.appPath, '');
        expect(harness.quits, 0);
      },
      variant: desktop,
    );

    testWidgets('nothing is offered to up-to-date or unstamped builds', (
      tester,
    ) async {
      for (final (version, app) in [
        ('2026.11.18-1700', '2026.11.18-1700'),
        ('dev', 'dev'),
        ('2026.11.18-1800', 'dev'),
      ]) {
        final harness = Harness(
          focusedOn('', version: version, latest: '2026.11.18-1700'),
        );
        harness.appVersion = app;
        await harness.show(tester);
        expect(find.text('Update'), findsNothing, reason: '$version / $app');
      }
    }, variant: desktop);

    testWidgets('an app-only release says agents keep working', (tester) async {
      final harness = Harness(
        focusedOn(
          '',
          version: '2026.11.18-1600',
          latest: '2026.11.18-1700',
          latestBackend: '2026.11.18-1600',
          working: true,
        ),
      );
      harness.appVersion = '2026.11.18-1600';
      await harness.show(tester);
      expect(
        find.textContaining('Only the app restarts; agents keep working.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Update'));
      await settle(tester);
      expect(harness.last.installUpdate.appPath, '/Applications/Factory.app');
    }, variant: desktop);

    testWidgets('a backend update tells working agents carry on', (
      tester,
    ) async {
      final harness = Harness(
        focusedOn(
          '',
          version: '2026.11.18-1600',
          latest: '2026.11.18-1700',
          working: true,
        ),
      );
      harness.appVersion = '2026.11.18-1700';
      await harness.show(tester);
      expect(
        find.textContaining('The working agent picks up where it left off.'),
        findsOneWidget,
      );
    }, variant: desktop);

    testWidgets('an undone update says why and offers to try again', (
      tester,
    ) async {
      const why =
          "Claktory 2026.11.18-1700 didn't start, so you're still on "
          '2026.11.18-1600. It stopped (exit status: 3).';
      final harness = Harness(
        focusedOn(
          '',
          version: '2026.11.18-1600',
          latest: '2026.11.18-1700',
          updateError: why,
        ),
      );
      harness.appVersion = '2026.11.18-1700';
      await harness.show(tester);
      expect(find.text(why), findsOneWidget);
      await tester.tap(find.text('Try again'));
      await settle(tester);
      expect(harness.last.hasInstallUpdate(), isTrue);
    }, variant: desktop);
  });
}
