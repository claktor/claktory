import 'package:factory_app/connections.dart';
import 'package:factory_app/loopback_forward.dart';
import 'package:factory_app/markdown.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/selection.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'tool_views_test.dart' show call;

/// A forwarded port that counts what the test says it carried.
class FakeForward implements PortForward {
  FakeForward(this.port);

  @override
  final int port;

  @override
  var connections = 0;

  @override
  var lastError = '';

  var stopped = false;

  /// The port on the Claktory's machine it goes to.
  int? target;

  @override
  Future<void> stop() async => stopped = true;
}

/// Forwards on a Claktory on another machine (unless [remote] is false),
/// the ports in [taken] refused as used on this device.
class Forwarder {
  Forwarder({this.remote = true, this.taken = const {}});

  bool remote;
  final Set<int> taken;
  final made = <FakeForward>[];

  late final forwards = LoopbackForwards(
    remote: () => remote,
    forward: (port) async {
      if (taken.contains(port)) {
        throw const ForwardFailure(ForwardProblem.taken);
      }
      final forward = FakeForward(port);
      made.add(forward);
      return forward;
    },
    forwardTo: (port, target) async {
      if (taken.contains(port)) {
        throw const ForwardFailure(ForwardProblem.taken);
      }
      final forward = FakeForward(port)..target = target;
      made.add(forward);
      return forward;
    },
    lifetime: const Duration(minutes: 10),
  );
}

const _claude =
    'https://claude.ai/oauth/authorize?code=true&client_id=x'
    '&redirect_uri=http%3A%2F%2Flocalhost%3A53692%2Fcallback&state=s';

final _code =
    'factory://100.64.0.2:7443?t=${'a' * 64}&fp=${'b' * 64}&name=mac-mini';

void main() {
  group('loopbackPorts', () {
    test('finds the ports a page comes back to', () {
      expect(loopbackPorts(Uri.parse(_claude)), [53692]);
      expect(
        loopbackPorts(
          Uri.parse(
            'https://auth.openai.com/oauth/authorize?'
            'redirect_uri=http://localhost:1455/auth/callback',
          ),
        ),
        [1455],
      );
      expect(
        loopbackPorts(
          Uri.parse(
            'https://openrouter.ai/auth?callback_url='
            '${Uri.encodeComponent('http://127.0.0.1:50123/oauth/callback/u')}',
          ),
        ),
        [50123],
      );
      expect(
        loopbackPorts(
          Uri.parse('https://a.test/x?redirect=http://[::1]:8976/'),
        ),
        [8976],
      );
      expect(
        loopbackPorts(
          Uri.parse('https://a.test/x?callback=http://localhost:9/'),
        ),
        [9],
      );
      // The page itself is on this device's loopback.
      expect(loopbackPorts(Uri.parse('http://localhost:8080/login')), [8080]);
    });

    test('ignores addresses that go elsewhere or name no port', () {
      expect(loopbackPorts(Uri.parse('https://claude.ai/login')), isEmpty);
      expect(
        loopbackPorts(
          Uri.parse('https://a.test/x?redirect_uri=https://b.test:8443/cb'),
        ),
        isEmpty,
      );
      expect(
        loopbackPorts(
          Uri.parse('https://a.test/x?redirect_uri=http://localhost/cb'),
        ),
        isEmpty,
      );
      expect(
        loopbackPorts(Uri.parse('https://a.test/x?state=http://localhost:1/')),
        isEmpty,
      );
      expect(
        loopbackPorts(
          Uri.parse('https://a.test/x?redirect_uri=myapp://localhost:3/'),
        ),
        isEmpty,
      );
    });
  });

  group('LoopbackForwards', () {
    test('forwards nothing for a Claktory on this machine, or a page '
        "that doesn't come back", () async {
      final forwarder = Forwarder(remote: false);
      expect(await forwarder.forwards.start(Uri.parse(_claude)), isNull);
      forwarder.remote = true;
      expect(
        await forwarder.forwards.start(Uri.parse('https://claude.ai/')),
        isNull,
      );
      expect(forwarder.made, isEmpty);
      expect(forwarder.forwards.active, isEmpty);
    });

    test('shares a port between flows and stops it with the last', () async {
      final forwarder = Forwarder();
      final forwards = forwarder.forwards;
      final a = (await forwards.start(Uri.parse(_claude)))!;
      final b = (await forwards.start(
        Uri.parse('https://b.test/?redirect_uri=http://localhost:53692/x'),
      ))!;
      expect(forwarder.made.map((f) => f.port), [53692]);
      expect(forwards.active, [a, b]);
      expect(a.working, isTrue);

      // Opening the same page again is the same flow.
      expect(await forwards.start(Uri.parse(_claude)), same(a));

      await forwards.stop(a);
      expect(forwarder.made.single.stopped, isFalse);
      await forwards.stop(b);
      expect(forwarder.made.single.stopped, isTrue);
      expect(forwards.active, isEmpty);
    });

    test('says which port is taken on this device', () async {
      final forwarder = Forwarder(taken: {53692});
      final flow = (await forwarder.forwards.start(Uri.parse(_claude)))!;
      expect(flow.working, isFalse);
      expect(flow.problems, {53692: ForwardProblem.taken});
      expect(
        forwardingProblem(forwarder.forwards, flow, 'mac-mini'),
        allOf(
          contains('is using port 53692'),
          contains("can't reach mac-mini"),
          contains('paste the address'),
        ),
      );
    });

    testWidgets('notices connections and what the Claktory says went wrong', (
      tester,
    ) async {
      final forwarder = Forwarder();
      final forwards = forwarder.forwards;
      var notified = 0;
      forwards.addListener(() => notified++);
      final flow = (await forwards.start(Uri.parse(_claude)))!;
      expect(
        forwardingState(forwards, flow, 'mac-mini'),
        'Waiting for you to finish in the browser…',
      );
      expect(forwardingProblem(forwards, flow, 'mac-mini'), isNull);

      notified = 0;
      forwarder.made.single.connections = 1;
      await tester.pump(const Duration(seconds: 1));
      expect(notified, 1);
      expect(forwards.served(flow), 1);
      expect(
        forwardingState(forwards, flow, 'mac-mini'),
        'Done in the browser. Passed on to mac-mini.',
      );

      forwarder.made.single.lastError =
          'Nothing is listening on port 53692 on mac-mini.';
      await tester.pump(const Duration(seconds: 1));
      expect(notified, 2);
      expect(
        forwardingProblem(forwards, flow, 'mac-mini'),
        'Nothing is listening on port 53692 on mac-mini.',
      );
      await tester.pump(const Duration(seconds: 5));
      expect(notified, 2);
      forwards.dispose();
    });

    testWidgets('stops after its lifetime, and later for a page opened again', (
      tester,
    ) async {
      final forwarder = Forwarder();
      final forwards = forwarder.forwards;
      await forwards.start(Uri.parse(_claude));
      await tester.pump(const Duration(minutes: 9));
      await forwards.start(Uri.parse(_claude));
      await tester.pump(const Duration(minutes: 9));
      expect(forwards.active, hasLength(1));
      expect(forwarder.made.single.stopped, isFalse);
      await tester.pump(const Duration(minutes: 1, seconds: 1));
      expect(forwards.active, isEmpty);
      expect(forwarder.made.single.stopped, isTrue);
    });

    test('opens the page after its ports listen', () async {
      final forwarder = Forwarder();
      final events = <String>[];
      final flow = await openInBrowser(
        Uri.parse(_claude),
        forwards: forwarder.forwards,
        launch: (url) async {
          events.add('opened with ${forwarder.made.length} forwarded');
          return true;
        },
      );
      expect(events, ['opened with 1 forwarded']);
      expect(flow!.ports, [53692]);
      // A taken port still opens the page: pasting works.
      final taken = Forwarder(taken: {53692});
      await openInBrowser(
        Uri.parse(_claude),
        forwards: taken.forwards,
        launch: (url) async {
          events.add('opened anyway');
          return true;
        },
      );
      expect(events.last, 'opened anyway');
    });
  });

  // Websites shared over pipes, forwarded from a port of this
  // device's own for as long as the share lasts.
  group('shared websites', () {
    test('a share has a port of its own here, the same each time, away '
        "from programs' usual ones", () {
      final port = sharePortFor('s1');
      expect(port, inInclusiveRange(20000, 29999));
      expect(sharePortFor('s1'), port);
      expect(sharePortFor('s2'), isNot(port));
      expect(
        withPort('http://s-abc.localhost:3000/__claktory_open?k=x', port),
        'http://s-abc.localhost:$port/__claktory_open?k=x',
      );
    });

    test('forwarded once, to its port there, the next port here when its '
        'own is taken; it outlives the flows and their Stop', () async {
      final own = sharePortFor('s1');
      final forwarder = Forwarder(taken: {own});
      final forwards = forwarder.forwards;
      expect(await forwards.forwardShare('s1', 3000), own + 1);
      expect(await forwards.forwardShare('s1', 3000), own + 1);
      expect(forwarder.made, hasLength(1));
      expect(forwarder.made.single.target, 3000);
      expect(forwards.sharePort('s1'), own + 1);
      await forwards.stopFlows();
      expect(forwarder.made.single.stopped, isFalse);
      expect(forwards.active, isEmpty, reason: "not one of the sidebar's");
      await forwards.stopAll();
      expect(forwarder.made.single.stopped, isTrue);
      expect(forwards.sharePort('s1'), isNull);
    });

    test('kept to the shares wanted: new ones start, gone ones stop, one '
        'whose port there moved starts again', () async {
      final forwarder = Forwarder();
      final forwards = forwarder.forwards;
      await forwards.keepShares({'s1': 3000, 's2': 5173});
      expect(forwards.sharePort('s1'), sharePortFor('s1'));
      expect(forwards.sharePort('s2'), sharePortFor('s2'));
      expect(forwarder.made, hasLength(2));
      await forwards.keepShares({'s1': 3001});
      expect(forwarder.made[0].stopped, isTrue);
      expect(forwarder.made[1].stopped, isTrue);
      expect(forwarder.made.last.target, 3001);
      expect(forwards.sharePort('s2'), isNull);
      await forwards.keepShares({});
      expect(forwarder.made.last.stopped, isTrue);
    });

    test('this Claktory on this machine: nothing to forward', () async {
      final forwarder = Forwarder(remote: false);
      await forwarder.forwards.keepShares({'s1': 3000});
      expect(forwarder.made, isEmpty);
    });

    test('no port free near its own: taken, said once tried', () async {
      final own = sharePortFor('s1');
      final forwarder = Forwarder(
        taken: {for (var i = 0; i < 20; i++) own + i},
      );
      await expectLater(
        forwarder.forwards.forwardShare('s1', 3000),
        throwsA(
          isA<ForwardFailure>().having(
            (failure) => failure.problem,
            'problem',
            ForwardProblem.taken,
          ),
        ),
      );
      expect(
        forwarder.forwards.shareProblem('s1')?.problem,
        ForwardProblem.taken,
      );
    });
  });

  group('links', () {
    late List<Uri> opened;
    setUp(() {
      opened = [];
      linkLauncher = (url) async {
        opened.add(url);
        return true;
      };
    });
    tearDown(() => linkLauncher = null);

    Future<void> showText(WidgetTester tester, Widget child) =>
        tester.pumpWidget(
          Directionality(
            textDirection: TextDirection.ltr,
            child: MediaQuery(
              data: const MediaQueryData(),
              child: Overlay(
                initialEntries: [OverlayEntry(builder: (_) => child)],
              ),
            ),
          ),
        );

    testWidgets('a markdown link opens', (tester) async {
      await showText(
        tester,
        const Markdown('Sign in at [the page](https://a.test/login) now.'),
      );
      await tester.tapOnText(find.textRange.ofSubstring('the page'));
      expect(opened, [Uri.parse('https://a.test/login')]);
    });

    testWidgets('a drag that starts on a link selects its text', (
      tester,
    ) async {
      SelectedContent? selected;
      await showText(
        tester,
        SelectableArea(
          onSelectionChanged: (content) => selected = content,
          child: const Markdown('Sign in at [the page](https://a.test/x) now.'),
        ),
      );
      final paragraph = tester.renderObject<RenderParagraph>(
        find.byType(RichText).first,
      );
      Offset at(int index) => paragraph.localToGlobal(
        paragraph
                .getBoxesForSelection(
                  TextSelection(baseOffset: index, extentOffset: index + 1),
                )
                .first
                .toRect()
                .centerLeft +
            const Offset(1, 0),
      );
      const text = 'Sign in at the page now.';
      final drag = await tester.startGesture(
        at(text.indexOf('page')),
        kind: PointerDeviceKind.mouse,
      );
      await tester.pump();
      await drag.moveTo(at(text.indexOf('now.') + 3));
      await tester.pump();
      await drag.up();
      await tester.pumpAndSettle();
      expect(selected?.plainText, 'page now');
      expect(opened, isEmpty);
    });

    testWidgets("a command's output opens its addresses, without the "
        'punctuation after them', (tester) async {
      await showText(
        tester,
        const CodeBlock(
          'Open (https://dash.test/oauth?x=1).\nThen wait.',
          links: true,
        ),
      );
      await tester.tapOnText(
        find.textRange.ofSubstring('https://dash.test/oauth?x=1'),
      );
      expect(opened, [Uri.parse('https://dash.test/oauth?x=1')]);
      opened.clear();
      await tester.tapOnText(find.textRange.ofSubstring('Then wait'));
      expect(opened, isEmpty);
    });

    testWidgets('code without links makes no recognizers', (tester) async {
      await showText(tester, const CodeBlock('ls -la', links: true));
      final text = tester.widget<Text>(find.text('ls -la'));
      expect(text.textSpan, isNull);
    });
  });

  group('in the app', () {
    late List<Uri> opened;
    setUp(() {
      opened = [];
      linkLauncher = (url) async {
        opened.add(url);
        return true;
      };
    });
    tearDown(() => linkLauncher = null);

    const wrangler =
        'https://dash.cloudflare.com/oauth2/auth?response_type=code'
        '&redirect_uri=http%3A%2F%2Flocalhost%3A8976%2Foauth%2Fcallback';

    /// Connected to mac-mini, with Mike's bash call that printed
    /// [wrangler]'s sign-in page.
    Future<Harness> showRemote(WidgetTester tester) async {
      final harness = Harness(Workspace());
      harness.backend.connectTo(parsePairingCode(_code));
      harness.backend.workspace.apply(
        Workspace(
          agents: [Agent(id: 'a1', name: 'Mike', modelAvailable: true)],
          selectedAgentId: 'a1',
          settings: Settings(userName: 'Matt'),
        ),
      );
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [
            call('bash', {
              'command': 'npx wrangler login',
            }, output: 'Opening a link in your default browser: $wrangler\n'),
          ],
        ),
      );
      await harness.show(tester);
      await settle(tester);
      return harness;
    }

    testWidgets("a CLI's sign-in link forwards its port until stopped", (
      tester,
    ) async {
      final harness = await showRemote(tester);
      final made = <FakeForward>[];
      harness.backend.forwarded = (port) async {
        final forward = FakeForward(port);
        made.add(forward);
        return forward;
      };
      await tester.tap(find.textContaining('npx wrangler login').first);
      await settle(tester);
      await tester.tapOnText(
        find.textRange.ofSubstring('https://dash.cloudflare.com'),
      );
      await tester.pump();
      expect(harness.backend.forwardedPorts, [8976]);
      expect(opened, [Uri.parse(wrangler)]);
      expect(find.text('Waiting for the browser…'), findsOneWidget);

      made.single.connections = 1;
      await tester.pump(const Duration(seconds: 1));
      expect(find.text('Passed on to mac-mini'), findsOneWidget);

      await tester.tap(find.text('Stop'));
      await tester.pump();
      expect(made.single.stopped, isTrue);
      expect(find.text('Passed on to mac-mini'), findsNothing);
      await harness.backend.forwards.stopAll();
    });

    testWidgets('a port taken on this device says so', (tester) async {
      final harness = await showRemote(tester);
      harness.backend.forwarded = (port) async =>
          throw const ForwardFailure(ForwardProblem.taken);
      await tester.tap(find.textContaining('npx wrangler login').first);
      await settle(tester);
      await tester.tapOnText(
        find.textRange.ofSubstring('https://dash.cloudflare.com'),
      );
      await tester.pump();
      expect(find.text("The browser can't reach mac-mini"), findsOneWidget);
      // The page opens all the same.
      expect(opened, [Uri.parse(wrangler)]);
      await harness.backend.forwards.stopAll();
      await tester.pump();
    });

    testWidgets('a sign-in in Settings waits for the browser, then ends', (
      tester,
    ) async {
      final harness = Harness(workspace(signedIn: false));
      harness.backend.connectTo(parsePairingCode(_code));
      final made = <FakeForward>[];
      harness.backend.forwarded = (port) async {
        final forward = FakeForward(port);
        made.add(forward);
        return forward;
      };
      await harness.show(tester);
      await harness.apply(tester, workspace(signedIn: false));
      harness.page.value = AppPage.settings;
      harness.backend.settingsSection.value = 'providers';
      await settle(tester);
      await tester.tap(find.text('Anthropic (Claude Pro/Max)'));
      await settle(tester);
      final flow = LoginFlow(
        providerId: 'anthropic',
        methodId: 'oauth',
        url: _claude,
        prompt: LoginPrompt(kind: LoginPromptKind.LOGIN_PROMPT_KIND_CODE),
      );
      await harness.apply(tester, workspace(signedIn: false, login: flow));
      await tester.pump();
      expect(harness.opened, [_claude]);
      expect(harness.backend.forwardedPorts, [53692]);
      expect(
        find.text('Waiting for you to finish in the browser…'),
        findsOneWidget,
      );
      // Settings says it; the sidebar doesn't again.
      expect(find.text('Stop'), findsNothing);

      // The browser came back and the backend signed in.
      made.single.connections = 1;
      await harness.apply(tester, workspace());
      expect(made.single.stopped, isTrue);
      expect(
        find.text('Waiting for you to finish in the browser…'),
        findsNothing,
      );
    });
  });
}
