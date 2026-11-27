// Websites shared over pipes (pipe-port-sharing.md's revised plan, Bruno
// 30866; Secco's contract 30935): Share a website… from a pipe's settings or
// a group's menu (a port the Claktory checks, a name, the pipes or the
// group, Claktory's one warning naming the port); the shares on a pipe in
// its settings, this side's with Stop and the other side's with Hide or
// Show; the other side's under the pipe's header with Open (Safari at
// 127.0.0.1), Copy address and Hide; and a pipe moved into or out of a
// group with shares, or a group deleted, saying what that changes.
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/devices.dart' show thisDeviceKind;
import 'package:factory_app/loopback_forward.dart';
import 'package:factory_app/mobile/share_browser.dart';
import 'package:factory_app/pipe_shares.dart';
import 'package:factory_app/pipes.dart' show pipeColor;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'selection_test.dart' show watchClipboard;

Pipe _pipe(
  String id,
  String name, {
  String group = '',
  PipeState state = PipeState.PIPE_STATE_OPEN,
}) => Pipe(
  id: id,
  name: name,
  projectId: 'p1',
  groupId: group,
  createdMs: Int64(int.parse(id.substring(1))),
  state: state,
  online: true,
  peer: PipePeer(ownerName: name.split(' · ').last),
);

PipeShare _theirs({
  bool live = true,
  bool hidden = false,
  String problem = '',
  bool unopened = false,
  int expires = 0,
}) => PipeShare(
  id: 's1',
  name: 'Dashboard',
  port: 3000,
  pipeIds: ['p3'],
  live: live,
  hidden: hidden,
  listenerProblem: problem,
  address: 'http://s-abc.localhost:3000/',
  fallbackAddress: 'http://127.0.0.1:3000/',
  unopened: unopened,
  expiresMs: Int64(expires),
);

PipeShare _mine({
  String group = 'g1',
  List<String> pipes = const ['p3', 'p4'],
  int expires = 0,
}) => PipeShare(
  id: 'm1',
  mine: true,
  name: 'Metrics',
  port: 8080,
  pipeIds: pipes,
  groupId: group,
  live: true,
  expiresMs: Int64(expires),
);

Workspace _workspace({
  List<PipeShare> shares = const [],
  String selected = '',
  PipeState state = PipeState.PIPE_STATE_OPEN,
}) => Workspace(
  projects: [Project(id: 'p1', name: 'Friend Feedback', path: '/w')],
  agents: [
    Agent(id: 'mike', name: 'Mike', projectId: 'p1', modelAvailable: true),
  ],
  pipeGroups: [
    PipeGroup(id: 'g1', projectId: 'p1', name: 'Friends', position: 0),
    PipeGroup(id: 'g2', projectId: 'p1', name: 'Launch', position: 1),
  ],
  pipes: [
    _pipe('p3', 'Feedback · Ana', group: 'g1', state: state),
    _pipe('p4', 'Feedback · Ben', group: 'g1'),
    _pipe('p5', 'Acme launch'),
  ],
  pipeShares: shares,
  selectedAgentId: selected.isEmpty ? 'mike' : '',
  selectedPipeId: selected,
);

/// The share form's fields: 0 the port, 1 the name.
Finder _field(int i) => find
    .descendant(
      of: find.byType(ShareWebsiteForm),
      matching: find.byType(EditableText),
    )
    .at(i);

Finder _row(String id) => find.byKey(ValueKey('pipe-row/$id'));
Finder _group(String id) => find.byKey(ValueKey('pipe-group/$id'));

Future<Harness> _show(
  WidgetTester tester,
  Workspace workspace, {
  Size size = const Size(1200, 1000),
  List<PipeMessage> messages = const [],
}) async {
  final harness = Harness(workspace);
  harness.backend.pipeChat.load(
    PipeTranscript(pipeId: workspace.selectedPipeId, messages: messages),
  );
  await harness.show(tester, size: size);
  await settle(tester);
  return harness;
}

SharePipeWebsite _shared(Harness harness) => harness.backend.sent
    .lastWhere((message) => message.hasSharePipeWebsite())
    .sharePipeWebsite;

void main() {
  test('the time a share has left, in the largest whole unit, rounded up', () {
    final now = DateTime(2026, 6, 1, 12);
    String? left(Duration d) => shareEnds(
      PipeShare(expiresMs: Int64(now.add(d).millisecondsSinceEpoch)),
      now,
    );
    expect(shareEnds(PipeShare(), now), isNull);
    expect(left(const Duration(seconds: 20)), 'Ends in 1 minute');
    expect(left(const Duration(minutes: 59, seconds: 1)), 'Ends in 1 hour');
    expect(
      left(const Duration(minutes: 58, seconds: 30)),
      'Ends in 59 minutes',
    );
    expect(left(const Duration(hours: 1)), 'Ends in 1 hour');
    expect(left(const Duration(hours: 24)), 'Ends in 24 hours');
    expect(left(const Duration(hours: 48)), 'Ends in 48 hours');
    expect(left(const Duration(days: 7)), 'Ends in 7 days');
    expect(left(-const Duration(seconds: 1)), 'Ending now');
  });

  late List<Uri> opened;
  setUp(() {
    opened = [];
    openSharePage = (url) async {
      opened.add(url);
      return true;
    };
    defaultBrowserIsSafari = () async => false;
  });

  group("the other side's website, under the pipe's header", () {
    testWidgets('says who shares what and where, and opens it in the '
        'browser with its one-time address', (tester) async {
      final harness = await _show(
        tester,
        _workspace(shares: [_theirs()], selected: 'p3'),
      );
      expect(find.text('Ana shares Dashboard'), findsOneWidget);
      expect(find.text('http://s-abc.localhost:3000/'), findsOneWidget);
      await tester.tap(find.text('Open'));
      await settle(tester);
      expect(harness.last.openPipeShare.shareId, 's1');
      expect(opened.single.toString(), startsWith('http://s-abc.localhost'));
    });

    testWidgets('in Safari it opens, and says, the 127.0.0.1 address', (
      tester,
    ) async {
      defaultBrowserIsSafari = () async => true;
      await _show(tester, _workspace(shares: [_theirs()], selected: 'p3'));
      expect(
        find.textContaining('http://127.0.0.1:3000/ (Safari'),
        findsOneWidget,
      );
      await tester.tap(find.text('Open'));
      await settle(tester);
      expect(opened.single.toString(), startsWith('http://127.0.0.1:3000/'));
    });

    testWidgets('Safari chosen meanwhile: Open asks again, and opens and '
        'shows the 127.0.0.1 address', (tester) async {
      await _show(tester, _workspace(shares: [_theirs()], selected: 'p3'));
      expect(find.text('http://s-abc.localhost:3000/'), findsOneWidget);
      defaultBrowserIsSafari = () async => true;
      await tester.tap(find.text('Open'));
      await settle(tester);
      expect(opened.single.toString(), startsWith('http://127.0.0.1:3000/'));
      expect(
        find.textContaining('http://127.0.0.1:3000/ (Safari'),
        findsOneWidget,
      );
    });

    // Squalo's W2: one not opened yet stands out, in the pipe card's wash
    // and border, until it is.
    testWidgets('not opened yet, it stands out until it is', (tester) async {
      final harness = await _show(
        tester,
        _workspace(shares: [_theirs(unopened: true)], selected: 'p3'),
      );
      BoxDecoration? decoration() =>
          tester
                  .widget<Container>(
                    find
                        .ancestor(
                          of: find.text('Ana shares Dashboard'),
                          matching: find.byType(Container),
                        )
                        .first,
                  )
                  .decoration
              as BoxDecoration?;
      expect(decoration()?.color, pipeColor.withValues(alpha: 0.06));
      harness.backend.workspace.apply(
        _workspace(shares: [_theirs()], selected: 'p3'),
      );
      await settle(tester);
      expect(decoration()?.color, isNull);
    });

    // W2: the chat's line announcing it has Open and Copy address too,
    // while the share lasts; then it's a plain line.
    group("the chat's line announcing it", () {
      final announced = PipeMessage(
        seq: Int64(1),
        timestampMs: Int64(
          DateTime(2026, 11, 25, 14, 2).millisecondsSinceEpoch,
        ),
        author: PipeAuthor(kind: PipeAuthorKind.PIPE_AUTHOR_KIND_CLAKTORY),
        text: 'Ana shares Dashboard.',
        shareId: 's1',
      );
      Finder onLine(Finder finder) =>
          find.descendant(of: find.byType(ShareAnnouncement), matching: finder);

      testWidgets('opens it and copies its address', (tester) async {
        final copied = watchClipboard(tester);
        final harness = await _show(
          tester,
          _workspace(shares: [_theirs()], selected: 'p3'),
          messages: [announced],
        );
        expect(onLine(find.textContaining('Ana shares Dashboard.')), findsOne);
        await tester.tap(onLine(find.text('Open')));
        await settle(tester);
        expect(harness.last.openPipeShare.shareId, 's1');
        expect(opened.single.toString(), startsWith('http://s-abc.localhost'));
        await tester.tap(onLine(find.text('Copy address')));
        await tester.pump();
        expect(copied, ['http://s-abc.localhost:3000/']);
        await tester.pump(const Duration(seconds: 3));
      });

      testWidgets('hidden or ended, the line alone', (tester) async {
        final harness = await _show(
          tester,
          _workspace(shares: [_theirs(hidden: true)], selected: 'p3'),
          messages: [announced],
        );
        expect(onLine(find.text('Open')), findsNothing);
        harness.backend.workspace.apply(_workspace(selected: 'p3'));
        await settle(tester);
        expect(onLine(find.textContaining('Ana shares Dashboard.')), findsOne);
        expect(onLine(find.text('Open')), findsNothing);
      });
    });

    testWidgets('one that ends says the time it has left, counting down '
        '(friend bug #46)', (tester) async {
      var now = DateTime(2026, 6, 1, 12);
      shareClock = () => now;
      addTearDown(() => shareClock = DateTime.now);
      await _show(
        tester,
        _workspace(
          shares: [
            _theirs(
              expires: now
                  .add(const Duration(minutes: 2))
                  .millisecondsSinceEpoch,
            ),
          ],
          selected: 'p3',
        ),
      );
      expect(find.text('Ends in 2 minutes'), findsOneWidget);
      now = now.add(const Duration(seconds: 75));
      await tester.pump(const Duration(seconds: 30));
      expect(find.text('Ends in 1 minute'), findsOneWidget);
    });

    testWidgets('Copy address copies it; Hide hides it', (tester) async {
      final copied = watchClipboard(tester);
      final harness = await _show(
        tester,
        _workspace(shares: [_theirs()], selected: 'p3'),
      );
      await tester.tap(find.text('Copy address'));
      await tester.pump();
      expect(copied, ['http://s-abc.localhost:3000/']);
      expect(find.text('Copied'), findsOneWidget);
      await tester.pump(const Duration(seconds: 3));
      await tester.tap(find.text('Hide'));
      await settle(tester);
      final hide = harness.last.hidePipeShare;
      expect((hide.shareId, hide.hidden), ('s1', true));
    });

    testWidgets('while the pipe is paused it waits, and nothing opens', (
      tester,
    ) async {
      final harness = await _show(
        tester,
        _workspace(
          shares: [_theirs(live: false)],
          selected: 'p3',
          state: PipeState.PIPE_STATE_PAUSED,
        ),
      );
      expect(find.text('Waits while the pipe is paused.'), findsOneWidget);
      await tester.tap(find.text('Open'));
      await settle(tester);
      expect(harness.backend.sent.any((m) => m.hasOpenPipeShare()), isFalse);
    });

    testWidgets("why this computer can't serve it", (tester) async {
      await _show(
        tester,
        _workspace(
          shares: [_theirs(problem: 'Port 3000 is in use on this Mac.')],
          selected: 'p3',
        ),
      );
      expect(find.text('Port 3000 is in use on this Mac.'), findsOneWidget);
    });

    testWidgets('a hidden one stays away; nor does this side see its own', (
      tester,
    ) async {
      await _show(
        tester,
        _workspace(shares: [_theirs(hidden: true), _mine()], selected: 'p3'),
      );
      expect(find.byType(ReceivedShare), findsNothing);
    });

    // #503 (Matt's MacBook paired to his Mac Studio): a computer showing a
    // Claktory on another machine opens it through that one, from a port of
    // its own, at the share's own host; a phone says it can't yet.
    group('showing a Claktory elsewhere', () {
      Future<Harness> elsewhere(
        WidgetTester tester, {
        Set<int> taken = const {},
      }) async {
        final harness = await _show(
          tester,
          _workspace(shares: [_theirs()], selected: 'p3'),
        );
        harness.backend.forwarded = (port) async {
          if (taken.contains(port)) {
            throw const ForwardFailure(ForwardProblem.taken);
          }
          return _Forward(port);
        };
        harness.backend.connectTo(
          parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac'),
        );
        harness.backend.workspace.apply(
          _workspace(shares: [_theirs()], selected: 'p3'),
        );
        await settle(tester);
        return harness;
      }

      testWidgets('on a Mac, Open forwards it from a port of its own, to the '
          "share's there, and opens its own host at that port", (tester) async {
        debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        final own = sharePortFor('s1');
        final harness = await elsewhere(tester, taken: {own});
        expect(find.text('http://s-abc.localhost:3000/'), findsNothing);
        await tester.tap(find.text('Open'));
        await settle(tester);
        expect(harness.last.openPipeShare.shareId, 's1');
        // Its own port was taken here: the next.
        expect(harness.backend.forwardedPorts, [own, own + 1]);
        expect(harness.backend.forwardedTargets.last, 3000);
        expect(
          opened.single.toString(),
          'http://s-abc.localhost:${own + 1}/__claktory_open?k=once',
        );
        // The address here says so, and Copy address copies it.
        expect(find.text('http://s-abc.localhost:${own + 1}/'), findsOneWidget);
        final copied = watchClipboard(tester);
        await tester.tap(find.text('Copy address'));
        await tester.pump();
        expect(copied, ['http://s-abc.localhost:${own + 1}/']);
        await tester.pump(const Duration(seconds: 3));
        // Opened again, the same forward.
        await tester.tap(find.text('Open'));
        await settle(tester);
        expect(harness.backend.forwardedPorts, [own, own + 1]);
        expect(opened, hasLength(2));
      });

      testWidgets('on a Mac, Copy address before any Open serves and forwards '
          'it first', (tester) async {
        debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        final harness = await elsewhere(tester);
        final copied = watchClipboard(tester);
        await tester.tap(find.text('Copy address'));
        await settle(tester);
        expect(harness.last.openPipeShare.shareId, 's1');
        expect(copied, ['http://s-abc.localhost:${sharePortFor('s1')}/']);
        expect(opened, isEmpty);
      });

      testWidgets('in Safari, at 127.0.0.1 and the port here', (tester) async {
        debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        defaultBrowserIsSafari = () async => true;
        await elsewhere(tester);
        await tester.tap(find.text('Open'));
        await settle(tester);
        expect(
          opened.single.toString(),
          'http://127.0.0.1:${sharePortFor('s1')}/__claktory_open?k=once',
        );
      });

      // Abbacchio's row 1350: cut off, it names the Claktory, not "the
      // backend", and the line goes once it's connected again.
      testWidgets("offline, Open says it can't reach the Claktory, and the "
          'line goes once it is back', (tester) async {
        debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        final harness = await elsewhere(tester);
        harness.backend.workspace.apply(
          _workspace(shares: [_theirs()], selected: 'p3')
            ..settings = Settings(name: 'Studio'),
        );
        await settle(tester);
        harness.backend
          ..refuse = 'Not connected to the backend.'
          ..connected.value = false;
        await tester.tap(find.text('Open'));
        await settle(tester);
        expect(find.text("Can't reach Studio right now."), findsOneWidget);
        expect(find.textContaining('backend'), findsNothing);
        harness.backend
          ..refuse = null
          ..connected.value = true;
        await settle(tester);
        expect(find.text("Can't reach Studio right now."), findsNothing);
      });

      testWidgets("a refusal that isn't the connection's stays after a "
          'reconnect', (tester) async {
        debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        final harness = await elsewhere(tester);
        harness.backend.refuse = 'The share ended.';
        await tester.tap(find.text('Open'));
        await settle(tester);
        expect(find.text('The share ended.'), findsOneWidget);
        harness.backend.connected.value = false;
        await tester.pump();
        harness.backend.connected.value = true;
        await settle(tester);
        expect(find.text('The share ended.'), findsOneWidget);
      });

      testWidgets("an old Claktory can't forward: it says so", (tester) async {
        debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        final harness = await elsewhere(tester);
        harness.backend.forwarded = (_) async =>
            throw const ForwardFailure(ForwardProblem.unsupported);
        await tester.tap(find.text('Open'));
        await settle(tester);
        expect(opened, isEmpty);
        expect(
          find.textContaining('needs an update before this Mac can open it'),
          // Once: the share's line and Open's failure are the same words.
          findsOneWidget,
        );
      });

      // W6: on Android it opens in a Custom Tab that keeps Claktory up,
      // at the share's own host and this device's port, never 127.0.0.1.
      group('on Android', () {
        late List<Uri> tabs;
        late String how;
        setUp(() {
          tabs = [];
          how = 'custom_tab';
          openShareTab = (url) async {
            if (how == 'no_browser' || how == 'bad_url') {
              throw PlatformException(code: how);
            }
            tabs.add(url);
            return how;
          };
        });

        testWidgets('Open forwards it and opens it in a tab; nothing to '
            'copy', (tester) async {
          final harness = await elsewhere(tester);
          expect(find.text('Copy address'), findsNothing);
          await tester.tap(find.text('Open'));
          await settle(tester);
          expect(harness.backend.forwardedTargets, [3000]);
          expect(
            tabs.single.toString(),
            'http://s-abc.localhost:${sharePortFor('s1')}/__claktory_open'
            '?k=once',
          );
          expect(opened, isEmpty, reason: 'not the system browser');
          expect(find.text(shareBrowserNote), findsNothing);
        });

        testWidgets('a site that failed while its tab was up says why once '
            'the person is back (row 655)', (tester) async {
          final harness = await elsewhere(tester);
          await tester.tap(find.text('Open'));
          await settle(tester);
          tester.binding.handleAppLifecycleStateChanged(
            AppLifecycleState.inactive,
          );
          tester.binding.handleAppLifecycleStateChanged(
            AppLifecycleState.hidden,
          );
          tester.binding.handleAppLifecycleStateChanged(
            AppLifecycleState.paused,
          );
          harness.backend.workspace.apply(
            _workspace(
              shares: [
                _theirs()
                  ..lastFailureMs = Int64(5000)
                  ..lastFailure = 'The shared website stopped answering.',
              ],
              selected: 'p3',
            ),
          );
          await settle(tester);
          expect(find.textContaining('stopped answering'), findsNothing);
          tester.binding.handleAppLifecycleStateChanged(
            AppLifecycleState.hidden,
          );
          tester.binding.handleAppLifecycleStateChanged(
            AppLifecycleState.inactive,
          );
          tester.binding.handleAppLifecycleStateChanged(
            AppLifecycleState.resumed,
          );
          await settle(tester);
          expect(
            find.text('The shared website stopped answering.'),
            findsOneWidget,
          );
        });

        testWidgets('in a browser that keeps nothing up, it says it stops '
            'loading when you leave', (tester) async {
          how = 'browser';
          await elsewhere(tester);
          await tester.tap(find.text('Open'));
          await settle(tester);
          expect(find.text(shareBrowserNote), findsOneWidget);
        });

        testWidgets('with no browser, it says so', (tester) async {
          how = 'no_browser';
          await elsewhere(tester);
          await tester.tap(find.text('Open'));
          await settle(tester);
          expect(
            find.textContaining("There's no browser on this"),
            findsOneWidget,
          );
        });

        testWidgets("an address it can't open names the website", (
          tester,
        ) async {
          how = 'bad_url';
          await elsewhere(tester);
          await tester.tap(find.text('Open'));
          await settle(tester);
          expect(
            find.text("Dashboard can't open on this $thisDeviceKind."),
            findsOneWidget,
          );
        });
      });

      // W6 on iPhone and iPad (#505): in Safari's view over the app,
      // which stays in front with the forward, at the share's own host.
      group('on an iPhone or iPad', () {
        late List<Uri> pages;
        late List<Uri?> fallbacks;
        late String how;
        MockStreamHandlerEventSink? sink;
        setUp(() {
          pages = [];
          how = 'in_app';
          sink = null;
          inAppShareBrowser.debugReset();
          fallbacks = [];
          openShareOver = (url, fallback) async {
            fallbacks.add(fallback);
            if (how == 'no_browser' || how == 'no_localhost') {
              throw PlatformException(code: how);
            }
            pages.add(url);
            sink?.success({'visible': true});
            return how;
          };
        });
        tearDown(inAppShareBrowser.debugReset);

        Future<Harness> iPhone(WidgetTester tester) async {
          debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
          addTearDown(() => debugDefaultTargetPlatformOverride = null);
          tester.binding.defaultBinaryMessenger.setMockStreamHandler(
            const EventChannel('factory/share-browser/events'),
            MockStreamHandler.inline(
              onListen: (_, events) {
                sink = events;
                events.success({'visible': false});
              },
            ),
          );
          addTearDown(
            () => tester.binding.defaultBinaryMessenger.setMockStreamHandler(
              const EventChannel('factory/share-browser/events'),
              null,
            ),
          );
          return elsewhere(tester);
        }

        testWidgets('Open forwards it and opens it over the app; nothing to '
            'copy', (tester) async {
          final harness = await iPhone(tester);
          expect(find.textContaining('isn’t available'), findsNothing);
          expect(find.text('Copy address'), findsNothing);
          expect(find.text('Hide'), findsOneWidget);
          await tester.tap(find.text('Open'));
          await settle(tester);
          expect(harness.backend.forwardedTargets, [3000]);
          expect(
            pages.single.toString(),
            'http://s-abc.localhost:${sharePortFor('s1')}/__claktory_open'
            '?k=once',
          );
          // Its fallback, should the system not find that address here:
          // the one the Claktory made, at this device's port.
          expect(
            fallbacks.single.toString(),
            'http://127.0.0.1:${sharePortFor('s1')}/__claktory_open?k=once',
          );
          expect(opened, isEmpty, reason: 'not Safari itself');
          expect(inAppShareBrowser.shows(harness.backend, 's1'), isTrue);
          // It stays in front: nothing says it stops when you leave.
          expect(find.text(shareBrowserNote), findsNothing);
        });

        testWidgets("a page that didn't load says why once the person is "
            'back', (tester) async {
          final harness = await iPhone(tester);
          await tester.tap(find.text('Open'));
          await settle(tester);
          sink!.success({'visible': true, 'loaded': false});
          await settle(tester);
          // Still on the page: nothing said yet.
          expect(find.textContaining("didn't open"), findsNothing);
          sink!.success({'visible': false});
          await settle(tester);
          expect(
            find.text("Dashboard didn't open. Try again."),
            findsOneWidget,
          );
          expect(inAppShareBrowser.shows(harness.backend, 's1'), isFalse);
        });

        // Zucchero row 655: a site that stopped answers with the Claktory's
        // own page, which Safari counts as loaded; the Claktory says it
        // failed on the share instead (last_failure).
        testWidgets('a site that failed while its page was up says why once '
            'the person is back', (tester) async {
          final harness = await iPhone(tester);
          await tester.tap(find.text('Open'));
          await settle(tester);
          harness.backend.workspace.apply(
            _workspace(
              shares: [
                _theirs()
                  ..lastFailureMs = Int64(5000)
                  ..lastFailure =
                      "Dashboard isn't answering on the other side.",
              ],
              selected: 'p3',
            ),
          );
          await settle(tester);
          // Still on the page: nothing said yet.
          expect(find.textContaining("isn't answering"), findsNothing);
          sink!.success({'visible': false});
          await settle(tester);
          expect(
            find.text("Dashboard isn't answering on the other side."),
            findsOneWidget,
          );
        });

        testWidgets('a failure from before the page opened, or one cleared '
            'while it was up, says nothing', (tester) async {
          final harness = await iPhone(tester);
          final failed = _theirs()
            ..lastFailureMs = Int64(5000)
            ..lastFailure = 'The shared website stopped answering.';
          harness.backend.workspace.apply(
            _workspace(shares: [failed], selected: 'p3'),
          );
          await settle(tester);
          await tester.tap(find.text('Open'));
          await settle(tester);
          sink!.success({'visible': false});
          await settle(tester);
          expect(find.textContaining('stopped answering'), findsNothing);
          // Open again: it loads, the failure clears.
          await tester.tap(find.text('Open'));
          await settle(tester);
          harness.backend.workspace.apply(
            _workspace(shares: [_theirs()], selected: 'p3'),
          );
          sink!.success({'visible': false});
          await settle(tester);
          expect(find.textContaining('stopped answering'), findsNothing);
        });

        testWidgets('offline by then, it names the Claktory, and the line goes '
            'once it is back', (tester) async {
          final harness = await iPhone(tester);
          harness.backend.workspace.apply(
            _workspace(shares: [_theirs()], selected: 'p3')
              ..settings = Settings(name: 'Studio'),
          );
          await settle(tester);
          await tester.tap(find.text('Open'));
          await settle(tester);
          sink!.success({'visible': true, 'loaded': false});
          harness.backend.connected.value = false;
          sink!.success({'visible': false});
          await settle(tester);
          expect(find.text("Can't reach Studio right now."), findsOneWidget);
          harness.backend.connected.value = true;
          await settle(tester);
          expect(find.text("Can't reach Studio right now."), findsNothing);
        });

        testWidgets("where the system can't find a share's own address, it "
            'says so and opens nothing else', (tester) async {
          how = 'no_localhost';
          final harness = await iPhone(tester);
          await tester.tap(find.text('Open'));
          await settle(tester);
          expect(
            find.text(
              "This $thisDeviceKind can't open shared websites on this "
              'network yet.',
            ),
            findsOneWidget,
          );
          expect(pages, isEmpty);
          expect(opened, isEmpty, reason: 'never 127.0.0.1 in Safari instead');
          expect(inAppShareBrowser.shows(harness.backend, 's1'), isFalse);
        });

        testWidgets("when it can't show the page, it says so", (tester) async {
          how = 'no_browser';
          final harness = await iPhone(tester);
          await tester.tap(find.text('Open'));
          await settle(tester);
          expect(
            find.text("Dashboard can't open on this $thisDeviceKind."),
            findsOneWidget,
          );
          expect(inAppShareBrowser.shows(harness.backend, 's1'), isFalse);
        });
      });
    });

    testWidgets('on a phone it fits', (tester) async {
      await _show(
        tester,
        _workspace(shares: [_theirs()], selected: 'p3'),
        size: const Size(390, 844),
      );
      expect(find.text('Ana shares Dashboard'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    // Abbacchio's row 1374: 27 websites shared (the most kept is 32) never
    // push the chat and its box out of reach; they scroll in their room.
    for (final (device, size) in [
      ('a Mac', const Size(1200, 860)),
      ('a phone', const Size(390, 844)),
    ]) {
      testWidgets('many shared on $device: they scroll, and the box stays', (
        tester,
      ) async {
        final shares = [
          for (var i = 0; i < 27; i++)
            _theirs()
              ..id = 's$i'
              ..name = 'Site ${8830 + i}'
              ..unopened = i.isEven,
        ];
        await _show(
          tester,
          _workspace(shares: shares, selected: 'p3'),
          size: size,
        );
        expect(tester.takeException(), isNull);
        final box = find.byType(EditableText).last;
        expect(box, findsOneWidget);
        expect(tester.getBottomLeft(box).dy, lessThanOrEqualTo(size.height));
        final strip = find.byType(PipeSharesStrip);
        expect(
          tester.getSize(strip).height,
          lessThanOrEqualTo(size.height * sharesStripShare),
        );
        // The last one is reached by scrolling the strip.
        final last = find.text('Ana shares Site 8856');
        await tester.dragUntilVisible(
          last,
          find.descendant(of: strip, matching: find.byType(Scrollable)),
          const Offset(0, -200),
        );
        await settle(tester);
        expect(tester.getTopLeft(last).dy, lessThan(size.height));
      });
    }
  });

  group("a pipe's settings", () {
    Future<Harness> settings(
      WidgetTester tester,
      List<PipeShare> shares,
    ) async {
      final harness = await _show(
        tester,
        _workspace(shares: shares, selected: 'p3'),
        size: const Size(1200, 2000),
      );
      await tester.tap(find.text('Feedback · Ana').last);
      await settle(tester);
      expect(find.text('Pipe settings'), findsOneWidget);
      return harness;
    }

    testWidgets('list the shares on it: this side stops its own, after '
        'asking', (tester) async {
      final harness = await settings(tester, [_mine()]);
      expect(find.text('Metrics · port 8080'), findsOneWidget);
      expect(
        find.text('With the group “Friends” · Until you stop it'),
        findsOneWidget,
      );
      await tester.tap(find.text('Stop'));
      await settle(tester);
      expect(find.text('Stop sharing Metrics?'), findsOneWidget);
      await tester.tap(find.text('Stop sharing'));
      await settle(tester);
      expect(harness.last.stopPipeShare.shareId, 'm1');
    });

    testWidgets("and the other side's, shown again from here once hidden", (
      tester,
    ) async {
      final harness = await settings(tester, [_theirs(hidden: true)]);
      expect(find.text('Dashboard, from Ana'), findsOneWidget);
      expect(find.text('Hidden from the top of the pipe'), findsOneWidget);
      await tester.tap(find.text('Show'));
      await settle(tester);
      final show = harness.last.hidePipeShare;
      expect((show.shareId, show.hidden), ('s1', false));
    });

    testWidgets("Hide changes the open settings as soon as the Claktory "
        "says, without reopening them (Abbacchio's row 1111)", (tester) async {
      final harness = await settings(tester, [_theirs()]);
      expect(find.text('At the top of the pipe'), findsOneWidget);
      // The settings' own, over the strip's under the header.
      await tester.tap(find.text('Hide').last);
      await settle(tester);
      expect(harness.last.hidePipeShare.hidden, isTrue);
      await harness.apply(
        tester,
        _workspace(shares: [_theirs(hidden: true)], selected: 'p3'),
      );
      await settle(tester);
      expect(find.text('Hidden from the top of the pipe'), findsOneWidget);
      expect(find.text('Show'), findsOneWidget);
    });

    // Squalo's W3: from a MacBook showing a Claktory on a Mac Studio, the
    // port shared is the Mac Studio's, and every sentence says so.
    testWidgets("from a computer showing a Claktory elsewhere, the sentences "
        "name the computer whose port it is", (tester) async {
      final harness = await settings(tester, []);
      harness.backend.connectTo(
        parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac'),
      );
      harness.backend.workspace.apply(
        _workspace(selected: 'p3')..settings = Settings(name: 'Studio'),
      );
      await settle(tester);
      expect(
        find.text('Share a website on Studio, such as a dashboard, with Ana.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      expect(
        find.text(
          'A website on Studio, such as a dashboard, that people on '
          'your pipes open in their own browser.',
        ),
        findsOneWidget,
      );
      harness.backend.pipeSharePort = (port) =>
          PipeSharePort(port: port, listening: false);
      await tester.enterText(_field(0), '3000');
      await tester.pump(const Duration(milliseconds: 500));
      await settle(tester);
      expect(
        find.text(
          'People on these pipes can use everything Studio serves on '
          'port 3000.',
        ),
        findsOneWidget,
      );
      expect(
        find.textContaining('Nothing on Studio answers on port 3000'),
        findsOneWidget,
      );
      expect(find.textContaining('this computer'), findsNothing);
    });

    testWidgets('Share a website… asks the port, the name and the pipes, '
        'says what it lets them do, and shares', (tester) async {
      final harness = await settings(tester, []);
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      expect(find.text('Share a website'), findsOneWidget);
      expect(
        find.text(
          'People on these pipes can use everything this computer serves on '
          'port ….',
        ),
        findsOneWidget,
      );
      harness.backend.pipeSharePort = (port) =>
          PipeSharePort(port: port, listening: false);
      await tester.enterText(_field(0), '3000');
      await tester.pump(const Duration(milliseconds: 500));
      await settle(tester);
      expect(harness.last.checkPipeSharePort.port, 3000);
      expect(
        find.text(
          'People on these pipes can use everything this computer serves on '
          'port 3000.',
        ),
        findsOneWidget,
      );
      expect(
        find.textContaining('Nothing on this computer answers on port 3000'),
        findsOneWidget,
      );
      // Begun from this pipe: it's chosen; another is added.
      await tester.tap(
        find.descendant(
          of: find.byType(ShareWebsiteForm),
          matching: find.textContaining('Acme launch', findRichText: true),
        ),
      );
      await settle(tester);
      await tester.enterText(_field(1), ' Dashboard ');
      await tester.tap(find.text('Share'));
      await settle(tester);
      final share = _shared(harness);
      expect(share.port, 3000);
      expect(share.name, 'Dashboard');
      expect(share.pipeIds.toSet(), {'p3', 'p5'});
      expect(share.groupId, isEmpty);
      // 24 hours unless changed (friend bug #46).
      expect(share.lastsS, Int64(86400));
      expect(find.text('Share a website'), findsNothing);
    });

    testWidgets('Share a website… asks how long it lasts: 1 hour, 24 hours, '
        '7 days or until it is stopped (friend bug #46)', (tester) async {
      final harness = await settings(tester, []);
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      expect(find.text('After 24 hours'), findsOneWidget);
      await tester.tap(find.text('After 24 hours'));
      await settle(tester);
      for (final choice in ['After 1 hour', 'After 7 days', 'Never']) {
        expect(find.text(choice), findsOneWidget);
      }
      await tester.tap(find.text('Never'));
      await settle(tester);
      expect(find.text('It stays shared until you stop it.'), findsOneWidget);
      await tester.enterText(_field(0), '3000');
      await tester.enterText(_field(1), 'Dashboard');
      await tester.pump(const Duration(milliseconds: 500));
      await tester.tap(find.text('Share'));
      await settle(tester);
      expect(_shared(harness).lastsS, Int64.ZERO);
    });

    testWidgets('a share of this side says the time it has left, and Change '
        'end… sets it again from now (friend bug #46)', (tester) async {
      final now = DateTime(2026, 6, 1, 12);
      shareClock = () => now;
      addTearDown(() => shareClock = DateTime.now);
      final harness = await settings(tester, [
        _mine(
          expires: now
              .add(const Duration(hours: 2, minutes: 30))
              .millisecondsSinceEpoch,
        ),
      ]);
      expect(
        find.text('With the group “Friends” · Ends in 3 hours'),
        findsOneWidget,
      );
      await tester.tap(find.text('Change end…'));
      await settle(tester);
      expect(
        find.text('When should Metrics stop being shared?'),
        findsOneWidget,
      );
      await tester.tap(find.text('After 7 days'));
      await settle(tester);
      final set = harness.last.setPipeShareExpiry;
      expect((set.shareId, set.lastsS), ('m1', Int64(604800)));
    });

    testWidgets('a time chosen after the share ended still goes, and the '
        "backend's refusal is said (#46, Abbacchio row 1551)", (tester) async {
      final now = DateTime(2026, 6, 1, 12);
      shareClock = () => now;
      addTearDown(() => shareClock = DateTime.now);
      final harness = await settings(tester, [
        _mine(
          expires: now.add(const Duration(minutes: 1)).millisecondsSinceEpoch,
        ),
      ]);
      await tester.tap(find.text('Change end…'));
      await settle(tester);
      // The share ends while the sheet is open: its row leaves.
      await harness.apply(tester, _workspace(shares: [], selected: 'p3'));
      expect(find.text('Metrics · port 8080'), findsNothing);
      harness.backend.refuse = "That website isn't shared any more.";
      await tester.tap(find.text('After 1 hour'));
      await settle(tester);
      expect(harness.last.setPipeShareExpiry.shareId, 'm1');
      expect(find.text("Couldn't change when Metrics ends"), findsOneWidget);
      expect(find.text("That website isn't shared any more."), findsOneWidget);
    });

    testWidgets('Stop after the share ended still says why it could not', (
      tester,
    ) async {
      final harness = await settings(tester, [_mine()]);
      await tester.tap(find.text('Stop'));
      await settle(tester);
      await harness.apply(tester, _workspace(shares: [], selected: 'p3'));
      harness.backend.refuse = "That website isn't shared any more.";
      await tester.tap(find.text('Stop sharing'));
      await settle(tester);
      expect(harness.last.stopPipeShare.shareId, 'm1');
      expect(find.text("Couldn't stop sharing Metrics"), findsOneWidget);
    });

    testWidgets('a port something answers on says what, so it is the one '
        'meant', (tester) async {
      final harness = await settings(tester, []);
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      harness.backend.pipeSharePort = (port) => PipeSharePort(
        port: port,
        listening: true,
        program: 'node',
        pid: 4121,
      );
      await tester.enterText(_field(0), '3000');
      await tester.pump(const Duration(milliseconds: 500));
      await settle(tester);
      expect(
        find.text('node (process 4121) answers on port 3000 on this computer.'),
        findsOneWidget,
      );
    });

    testWidgets("a port the Claktory won't share says why, and Share waits", (
      tester,
    ) async {
      final harness = await settings(tester, []);
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      harness.backend.pipeSharePort = (port) =>
          PipeSharePort(port: port, problem: "Port 7420 is Claktory's own.");
      await tester.enterText(_field(0), '7420');
      await tester.pump(const Duration(milliseconds: 500));
      await settle(tester);
      expect(find.text("Port 7420 is Claktory's own."), findsOneWidget);
      await tester.enterText(_field(1), 'Claktory');
      await tester.tap(find.text('Share'));
      await settle(tester);
      expect(harness.backend.sent.any((m) => m.hasSharePipeWebsite()), isFalse);
    });

    testWidgets('what the form said of the port goes as a port is typed', (
      tester,
    ) async {
      await settings(tester, []);
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      await tester.tap(find.text('Share'));
      await settle(tester);
      const said = 'Enter a port from 1 to 65535, such as 3000.';
      expect(find.text(said), findsOneWidget);
      await tester.enterText(_field(0), '3000');
      await tester.pump();
      expect(find.text(said), findsNothing);
      await tester.pump(const Duration(milliseconds: 500));
      await settle(tester);
    });

    testWidgets('a refusal says why and the form stays', (tester) async {
      final harness = await settings(tester, []);
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      await tester.enterText(_field(0), '3000');
      await tester.enterText(_field(1), 'Dashboard');
      harness.backend.refuse = 'That pipe closed.';
      await tester.tap(find.text('Share'));
      await settle(tester);
      expect(find.text('That pipe closed.'), findsOneWidget);
      expect(find.text('Share a website'), findsOneWidget);
    });
  });

  group('pipe groups', () {
    testWidgets("a group's menu shares a website with the group, and stops "
        'the ones that follow it', (tester) async {
      final harness = await _show(tester, _workspace(shares: [_mine()]));
      await tester.tap(_group('g1'), buttons: kSecondaryButton);
      await settle(tester);
      expect(find.text('Stop sharing Metrics…'), findsOneWidget);
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      expect(find.text('The group “Friends”'), findsOneWidget);
      expect(
        find.textContaining('Its pipes now: Feedback · Ana, Feedback · Ben.'),
        findsOneWidget,
      );
      await tester.enterText(_field(0), '3000');
      await tester.enterText(_field(1), 'Dashboard');
      await tester.tap(find.text('Share'));
      await settle(tester);
      final share = _shared(harness);
      expect(share.groupId, 'g1');
      expect(share.pipeIds, isEmpty);
    });

    testWidgets('moving a pipe into a group with shares says what it will '
        'also get, and asks', (tester) async {
      final harness = await _show(tester, _workspace(shares: [_mine()]));
      await tester.tap(_row('p5'), buttons: kSecondaryButton);
      await settle(tester);
      await tester.tap(find.text('Move to group…'));
      await settle(tester);
      await tester.tap(find.text('Friends').last);
      await settle(tester);
      expect(find.text('Move Acme launch to Friends?'), findsOneWidget);
      expect(find.text('This pipe will also get: Metrics.'), findsOneWidget);
      expect(harness.backend.sent.any((m) => m.hasSetPipeGroup()), isFalse);
      await tester.tap(find.text('Move'));
      await settle(tester);
      final moved = harness.last.setPipeGroup;
      expect((moved.pipeId, moved.groupId), ('p5', 'g1'));
    });

    testWidgets('moving one out says what it will stop getting; Cancel '
        'leaves it', (tester) async {
      final harness = await _show(tester, _workspace(shares: [_mine()]));
      await tester.tap(_row('p4'), buttons: kSecondaryButton);
      await settle(tester);
      await tester.tap(find.text('Remove from group'));
      await settle(tester);
      expect(
        find.text('This pipe will stop getting: Metrics.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Cancel'));
      await settle(tester);
      expect(harness.backend.sent.any((m) => m.hasSetPipeGroup()), isFalse);
    });

    testWidgets('a move that changes no share goes at once', (tester) async {
      final harness = await _show(tester, _workspace(shares: [_mine()]));
      await tester.tap(_row('p5'), buttons: kSecondaryButton);
      await settle(tester);
      await tester.tap(find.text('Move to group…'));
      await settle(tester);
      await tester.tap(find.text('Launch').last);
      await settle(tester);
      expect(harness.last.setPipeGroup.groupId, 'g2');
    });

    testWidgets('deleting a group says its websites stay with its pipes', (
      tester,
    ) async {
      await _show(tester, _workspace(shares: [_mine()]));
      await tester.tap(_group('g1'), buttons: kSecondaryButton);
      await settle(tester);
      await tester.tap(find.text('Delete group…'));
      await settle(tester);
      expect(
        find.text(
          'Its pipes stay, just outside any group. Metrics stays shared with '
          'its pipes.',
        ),
        findsOneWidget,
      );
    });
  });
}

class _Forward implements PortForward {
  _Forward(this.port);

  @override
  final int port;

  @override
  int get connections => 0;

  @override
  String get lastError => '';

  @override
  Future<void> stop() async {}
}
