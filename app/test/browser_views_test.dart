// The person's live views of an agent's browser in the app: Watch browser
// in its ⋯ and in Settings → Claktory's browser, while the browser is open,
// showing it beside the conversation (a page on a phone) without input;
// and a check request's card, whose Solve check opens the tab, live, for
// the person: Esc leaves the request waiting, Done and Can't do this
// answer it. With SHOTS set, each is drawn, dark, to a PNG there:
//   SHOTS=/tmp/views flutter test test/browser_views_test.dart
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/errands/browser_settings.dart'
    show AgentBrowserSettings;
import 'package:factory_app/errands/live_view.dart' show LiveScreen;
import 'package:factory_app/errands/tab_screens.dart';
import 'package:factory_app/errands/tab_view.dart';
import 'package:factory_app/mobile/device.dart';
import 'package:factory_app/panes.dart' show browserKind, withBrowser;
import 'package:factory_app/layout.dart' show chatOnly, leaves;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

Workspace _workspace({
  AgentBrowserState state = AgentBrowserState.AGENT_BROWSER_STATE_DRIVEN,
  bool pane = false,
  bool check = true,
  String name = 'Yukako',
}) => Workspace(
  projects: [Project(id: 'p1', name: 'Social')],
  agents: [
    Agent(
      id: 'yukako',
      name: name,
      role: 'Social',
      color: 'pink',
      projectId: 'p1',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      waitingSeqs: [if (check) Int64(4)],
      extensions: [AgentExtension(id: 'browser', enabled: true)],
      layout: pane ? withBrowser(chatOnly()) : null,
    ),
  ],
  extensions: [
    Extension(
      id: 'browser',
      name: 'Browser',
      icon: 'globe',
      description: "Uses websites in Claktory's browser.",
    ),
  ],
  agentBrowser: AgentBrowser(state: AgentBrowserState.AGENT_BROWSER_STATE_WARM),
  agentBrowsers: [AgentBrowser(agentId: 'yukako', state: state)],
  selectedAgentId: 'yukako',
  vault: VaultState(exists: true, unlocked: true),
);

Entry _check() => Entry(
  seq: Int64(4),
  timestampMs: Int64(1004),
  askPerson: AskPerson(
    id: 'ask4',
    title: 'Solve a check on x.com',
    body:
        '"X wants to know I’m not a robot before it shows the post."\n\n'
        "Yukako couldn't get past a check that it's a person on x.com.",
    state: AskPersonState.ASK_PERSON_STATE_PENDING,
    revision: 1,
    handoff: ErrandHandoff(
      errandId: 'e1',
      kind: ErrandHandoffKind.ERRAND_HANDOFF_KIND_CAPTCHA,
      what: 'X wants to know I’m not a robot before it shows the post.',
      origin: 'https://x.com',
      url: 'https://x.com/i/flow/login',
      domain: 'x.com',
      seq: Int64(4),
    ),
  ),
);

/// A page as the browser would send it: white, with a check box.
Future<ui.Image> _page(Uint8List bytes) {
  const width = 960, height = 600;
  final recorder = ui.PictureRecorder();
  final canvas = ui.Canvas(recorder);
  canvas.drawRect(
    const Rect.fromLTWH(0, 0, 960, 600),
    ui.Paint()..color = const Color(0xFFFFFFFF),
  );
  canvas.drawRect(
    const Rect.fromLTWH(0, 0, 960, 56),
    ui.Paint()..color = const Color(0xFFF2F2F2),
  );
  canvas.drawRect(
    const Rect.fromLTWH(300, 240, 360, 72),
    ui.Paint()
      ..color = const Color(0xFFD9D9D9)
      ..style = ui.PaintingStyle.stroke
      ..strokeWidth = 2,
  );
  canvas.drawRect(
    const Rect.fromLTWH(320, 260, 32, 32),
    ui.Paint()
      ..color = const Color(0xFF8C8C8C)
      ..style = ui.PaintingStyle.stroke
      ..strokeWidth = 2,
  );
  final words =
      ui.ParagraphBuilder(ui.ParagraphStyle(fontFamily: 'Geist', fontSize: 18))
        ..pushStyle(ui.TextStyle(color: const Color(0xFF1A1A1A)))
        ..addText('Verify you are human');
  final paragraph = words.build()
    ..layout(const ui.ParagraphConstraints(width: 280));
  canvas.drawParagraph(paragraph, const Offset(368, 264));
  return recorder.endRecording().toImage(width, height);
}

/// Claktory's side of the views: what the app sends, and frames to send it.
class _Claktory {
  _Claktory(Harness harness) {
    views = AgentTabViews(
      send: (message) async => sent.add(message),
      messages: messages.stream,
      connected: harness.backend.connected,
      decode: _page,
    );
    AgentTabViews.attach(harness.backend, views);
  }

  final sent = <ClientMessage>[];
  final messages = StreamController<ServerMessage>.broadcast(sync: true);
  late final AgentTabViews views;

  List<WatchAgentTab> get watches => [
    for (final message in sent)
      if (message.hasWatchAgentTab()) message.watchAgentTab,
  ];

  List<AgentTabInput> get inputs => [
    for (final message in sent)
      if (message.hasAgentTabInput()) message.agentTabInput,
  ];

  /// A frame of [request]'s tab (0: the browser watched), drawn.
  Future<void> frame(
    WidgetTester tester, {
    int request = 0,
    String title = 'Just a moment…',
  }) async {
    await tester.runAsync(() async {
      messages.add(
        ServerMessage(
          agentTabFrame: AgentTabFrame(
            agentId: 'yukako',
            requestSeq: Int64(request),
            frameSeq: Int64(1),
            jpeg: [1],
            width: 960,
            height: 600,
            site: 'x.com',
            origin: 'https://x.com',
            title: title,
          ),
        ),
      );
      await Future<void>.delayed(const Duration(milliseconds: 50));
    });
    await pumpFrames(tester);
  }

  Future<void> note(WidgetTester tester, String message) async {
    messages.add(
      ServerMessage(
        agentTabNote: AgentTabNote(
          agentId: 'yukako',
          requestSeq: Int64(4),
          message: message,
        ),
      ),
    );
    await pumpFrames(tester);
  }

  Future<void> end(WidgetTester tester, AgentTabViewEnd reason) async {
    messages.add(
      ServerMessage(
        agentTabViewEnded: AgentTabViewEnded(
          agentId: 'yukako',
          requestSeq: Int64(4),
          reason: reason,
        ),
      ),
    );
    await pumpFrames(tester);
  }
}

Future<void> _shot(WidgetTester tester, String name) async {
  expect(tester.takeException(), isNull);
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

enum _Device { mac, iphone, ipad }

Size _size(_Device device) => switch (device) {
  _Device.mac => const Size(1200, 800),
  _Device.iphone => const Size(393, 852),
  _Device.ipad => const Size(1180, 820),
};

Iterable<AnswerAsk> _answers(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasAnswerAsk()) message.answerAsk,
];

void main() {
  setUpAll(loadGeist);
  setUp(() {
    final dispatcher = TestWidgetsFlutterBinding.instance.platformDispatcher;
    dispatcher.platformBrightnessTestValue = Brightness.dark;
    addTearDown(dispatcher.clearPlatformBrightnessTestValue);
    WidgetsApp.debugAllowBannerOverride = false;
    addTearDown(() => WidgetsApp.debugAllowBannerOverride = true);
  });

  /// Yukako's conversation on [device], with the check's card unless
  /// [check] is false.
  Future<(Harness, _Claktory)> open(
    WidgetTester tester,
    _Device device, {
    AgentBrowserState state = AgentBrowserState.AGENT_BROWSER_STATE_DRIVEN,
    bool pane = false,
    bool check = true,
    String name = 'Yukako',
  }) async {
    debugDefaultTargetPlatformOverride = device == _Device.mac
        ? TargetPlatform.macOS
        : TargetPlatform.iOS;
    if (device == _Device.ipad) {
      debugDeviceCapabilities = const DeviceCapabilities(idiom: Idiom.pad);
    }
    final harness = Harness(
      _workspace(state: state, pane: pane, check: check, name: name),
    );
    final claktory = _Claktory(harness);
    harness.backend.transcript.load(
      Transcript(
        agentId: 'yukako',
        entries: [
          Entry(
            seq: Int64(2),
            timestampMs: Int64(1000),
            assistant: AssistantText(
              text: "X shows a check before the post. It's yours to solve.",
            ),
          ),
          if (check) _check(),
        ],
      ),
    );
    await harness.show(tester, size: _size(device));
    await pumpFrames(tester);
    return (harness, claktory);
  }

  void reset() {
    debugDefaultTargetPlatformOverride = null;
    debugDeviceCapabilities = null;
  }

  testWidgets("⋯ has Watch browser while Yukako's browser is open, and it "
      'opens a pane beside the conversation', (tester) async {
    try {
      final (harness, _) = await open(tester, _Device.mac, check: false);
      await tester.tap(find.bySemanticsLabel('More options for Yukako'));
      await settle(tester);
      await tester.tap(find.text('Watch browser'));
      await settle(tester);
      final layout = harness.backend.sent.lastWhere(
        (message) => message.hasSetLayout(),
      );
      expect(layout.setLayout.agentId, 'yukako');
      expect([
        for (final leaf in leaves(layout.setLayout.root)) leaf.leaf.kind,
      ], contains(browserKind));
    } finally {
      reset();
    }
  });

  testWidgets('⋯ has no Watch browser while the browser is closed', (
    tester,
  ) async {
    try {
      await open(
        tester,
        _Device.mac,
        check: false,
        state: AgentBrowserState.AGENT_BROWSER_STATE_OFF,
      );
      await tester.tap(find.bySemanticsLabel('More options for Yukako'));
      await settle(tester);
      expect(find.text('Agent settings…'), findsOneWidget);
      expect(find.text('Watch browser'), findsNothing);
    } finally {
      reset();
    }
  });

  testWidgets("a phone's ⋯ has Watch browser, which opens it as a page", (
    tester,
  ) async {
    try {
      final (_, claktory) = await open(tester, _Device.iphone, check: false);
      await tester.tap(find.bySemanticsLabel(RegExp('^More options for ')));
      await pumpFrames(tester);
      await tester.tap(find.text('Watch browser'));
      await pumpFrames(tester);
      expect(find.byType(BrowserWatch), findsOneWidget);
      expect(claktory.watches.single.requestSeq, Int64.ZERO);
      await claktory.frame(tester);
      await _shot(tester, 'watch-iphone');
    } finally {
      reset();
    }
  });

  testWidgets("Settings → Claktory's browser has Watch while it's open", (
    tester,
  ) async {
    try {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      final harness = Harness(_workspace(check: false));
      _Claktory(harness);
      await harness.show(
        tester,
        size: const Size(900, 1000),
        home: AgentBrowserSettings(
          workspace: harness.backend.workspace.workspace,
        ),
      );
      await pumpFrames(tester);
      await tester.tap(find.text('Watch'));
      await pumpFrames(tester);
      expect(
        harness.backend.sent.where((message) => message.hasSetLayout()),
        hasLength(1),
      );
    } finally {
      reset();
    }
  });

  testWidgets("Settings → Claktory's browser has no Watch while it's closed", (
    tester,
  ) async {
    try {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      final closed = Harness(
        _workspace(
          check: false,
          state: AgentBrowserState.AGENT_BROWSER_STATE_OFF,
        ),
      );
      _Claktory(closed);
      await closed.show(
        tester,
        size: const Size(900, 1000),
        home: AgentBrowserSettings(
          workspace: closed.backend.workspace.workspace,
        ),
      );
      await pumpFrames(tester);
      expect(find.text('Watch'), findsNothing);
    } finally {
      reset();
    }
  });

  for (final device in [_Device.mac, _Device.ipad]) {
    testWidgets('the browser pane shows the page and takes no input, '
        '${device.name}', (tester) async {
      try {
        final (_, claktory) = await open(
          tester,
          device,
          pane: true,
          check: false,
        );
        expect(claktory.watches.single.requestSeq, Int64.ZERO);
        expect(find.text('Starting…'), findsOneWidget);
        await claktory.frame(tester);
        expect(find.text('The page says “Just a moment…”'), findsOneWidget);
        await tester.tap(find.byType(BrowserWatch));
        await pumpFrames(tester);
        expect(claktory.inputs, isEmpty);
        await _shot(tester, 'watch-${device.name}');
      } finally {
        reset();
      }
    });
  }

  for (final device in _Device.values) {
    final at = device.name;
    testWidgets('the check card, $at', (tester) async {
      try {
        await open(tester, device);
        expect(find.text('Yukako needs your help'), findsWidgets);
        expect(find.text('Yukako waits until you solve it.'), findsWidgets);
        expect(find.text('Solve check'), findsWidgets);
        expect(
          find.textContaining('X wants to know I’m not a robot'),
          findsOneWidget,
          reason: "the agent's words once, as its own: not the body again",
        );
        expect(find.textContaining("couldn't get past"), findsNothing);
        await _shot(tester, 'check-card-$at');
      } finally {
        reset();
      }
    });

    testWidgets('Solve check opens the tab with input; Done answers, $at', (
      tester,
    ) async {
      try {
        final (harness, claktory) = await open(tester, device);
        await tester.tap(find.text('Solve check').last);
        await pumpFrames(tester);
        expect(find.byType(CheckView), findsOneWidget);
        final watch = claktory.watches.last;
        expect(watch.requestSeq, Int64(4));
        expect(watch.on, isTrue);
        await claktory.frame(tester, request: 4);
        expect(
          find.descendant(
            of: find.byType(CheckView),
            matching: find.text('Yukako waits until you solve it.'),
          ),
          findsNothing,
          reason: 'the card says it; the view only shows the page',
        );
        expect(find.text('The page says “Just a moment…”'), findsOneWidget);
        await _shot(tester, 'check-view-$at');

        await tester.tapAt(
          tester.getCenter(
            find.descendant(
              of: find.byType(LiveScreen),
              matching: find.byType(RawImage),
            ),
          ),
        );
        await tester.pump(const Duration(milliseconds: 400));
        await pumpFrames(tester);
        expect(claktory.inputs, isNotEmpty);
        expect(
          claktory.inputs.first.events.first.pointer.kind,
          AgentBrowserPointerKind.AGENT_BROWSER_POINTER_KIND_DOWN,
        );

        await tester.tap(find.text('Done'));
        await pumpFrames(tester);
        final answer = _answers(harness).single;
        expect(answer.seq, Int64(4));
        expect(answer.dismiss, isFalse);
        expect(find.byType(CheckView), findsNothing);
      } finally {
        reset();
      }
    });
  }

  testWidgets("Esc leaves the request waiting; Can't do this dismisses it", (
    tester,
  ) async {
    try {
      final (harness, claktory) = await open(tester, _Device.mac);
      await tester.tap(find.text('Solve check').last);
      await pumpFrames(tester);
      await claktory.frame(tester, request: 4);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await pumpFrames(tester);
      expect(find.byType(CheckView), findsNothing);
      expect(_answers(harness), isEmpty);
      expect(claktory.inputs, isEmpty, reason: "Esc is Claktory's");

      await tester.tap(find.text('Solve check').last);
      await pumpFrames(tester);
      await tester.tap(find.text("Can't do this").last);
      await pumpFrames(tester);
      expect(_answers(harness).single.dismiss, isTrue);
      expect(find.byType(CheckView), findsNothing);
    } finally {
      reset();
    }
  });

  testWidgets('a check answered elsewhere closes its view', (tester) async {
    try {
      final (_, claktory) = await open(tester, _Device.mac);
      await tester.tap(find.text('Solve check').last);
      await pumpFrames(tester);
      expect(find.byType(CheckView), findsOneWidget);
      await claktory.end(
        tester,
        AgentTabViewEnd.AGENT_TAB_VIEW_END_REQUEST_ENDED,
      );
      expect(find.byType(CheckView), findsNothing);
    } finally {
      reset();
    }
  });

  testWidgets('the tab leaving the site dims it and keeps the answers', (
    tester,
  ) async {
    try {
      final (_, claktory) = await open(tester, _Device.mac);
      await tester.tap(find.text('Solve check').last);
      await pumpFrames(tester);
      await claktory.frame(tester, request: 4);
      await claktory.end(tester, AgentTabViewEnd.AGENT_TAB_VIEW_END_LEFT_SITE);
      expect(
        find.text("The page left x.com, so you can't use it here any more."),
        findsOneWidget,
      );
      expect(find.text('Done'), findsOneWidget);
      expect(find.text("Can't do this"), findsWidgets);
      await _shot(tester, 'check-left-mac');
    } finally {
      reset();
    }
  });

  test("the page's title shows what could reorder or break it, and only "
      'so much of it', () {
    expect(
      pageTitleShown('Sign in\u202Emoc.live\nnow'),
      'Sign in\\u{202E}moc.live\\u{000A}now',
    );
    expect(pageTitleShown('a\u200Bb\tc'), 'a\\u{200B}b\\u{0009}c');
    expect(pageTitleShown('x' * 200), '${'x' * pageTitleMost}…');
    expect(pageTitleShown('Just a moment…'), 'Just a moment…');
  });

  testWidgets("the page's title stays inside its quotes", (tester) async {
    try {
      final (_, claktory) = await open(
        tester,
        _Device.mac,
        pane: true,
        check: false,
      );
      await claktory.frame(tester, title: 'Pay\u202E\nhere');
      expect(
        find.text('The page says “Pay\\u{202E}\\u{000A}here”'),
        findsOneWidget,
      );
    } finally {
      reset();
    }
  });

  /// The site's text is drawn whole within [within]: on screen, uncut.
  void wholeSite(WidgetTester tester, Finder within) {
    final site = find.descendant(of: within, matching: find.text('x.com'));
    expect(site, findsWidgets);
    final paragraph = tester.renderObject<RenderParagraph>(site.first);
    expect(paragraph.didExceedMaxLines, isFalse);
    final painter = TextPainter(
      text: paragraph.text,
      textDirection: TextDirection.ltr,
      textScaler: paragraph.textScaler,
    )..layout();
    expect(paragraph.size.width, greaterThanOrEqualTo(painter.width - 0.5));
    painter.dispose();
    final screen = tester.view.physicalSize / tester.view.devicePixelRatio;
    expect(tester.getRect(site.first).right, lessThanOrEqualTo(screen.width));
  }

  final longName = 'Y' * 60;
  for (final device in [_Device.mac, _Device.iphone]) {
    testWidgets('a 60-character name never cuts the site, ${device.name}', (
      tester,
    ) async {
      try {
        final (_, claktory) = await open(
          tester,
          device,
          name: longName,
          pane: device == _Device.mac,
        );
        if (device == _Device.iphone) {
          await tester.tap(find.text('Solve check').last);
          await pumpFrames(tester);
          await claktory.frame(tester, request: 4);
          wholeSite(tester, find.byType(CheckView));
          expect(
            find.descendant(
              of: find.byType(CheckView),
              matching: find.byWidgetPredicate(
                (widget) =>
                    widget is RichText &&
                    widget.text.toPlainText() == 'Solve a check on x.com' &&
                    widget.maxLines == null,
              ),
            ),
            findsOneWidget,
            reason: 'the heading wraps rather than cut the site',
          );
          await _shot(tester, 'check-view-long-name-iphone');
        } else {
          await claktory.frame(tester);
          wholeSite(tester, find.byType(BrowserWatch));
          await _shot(tester, 'watch-long-name-mac');
        }
      } finally {
        reset();
      }
    });
  }

  testWidgets("a phone's check view zooms: a double tap to 2.5× and back; a "
      'tap still clicks', (tester) async {
    try {
      final (_, claktory) = await open(tester, _Device.iphone);
      await tester.tap(find.text('Solve check').last);
      await pumpFrames(tester);
      await claktory.frame(tester, request: 4);
      final screen = find.byType(LiveScreen);
      final zoom = tester.widget<InteractiveViewer>(
        find.descendant(of: screen, matching: find.byType(InteractiveViewer)),
      );
      final top = tester.getTopLeft(screen);
      // The frame, 960×600, fitted to the phone's width from the top.
      final width = tester.getSize(screen).width;
      final inFrame = top + Offset(width / 2, width * 600 / 960 / 2);
      Future<void> doubleTap() async {
        await tester.tapAt(inFrame);
        await tester.pump(const Duration(milliseconds: 50));
        await tester.tapAt(inFrame);
        await pumpFrames(tester);
      }

      await doubleTap();
      expect(
        zoom.transformationController!.value.getMaxScaleOnAxis(),
        closeTo(2.5, 0.01),
      );
      expect(claktory.inputs, isEmpty, reason: 'a double tap only zooms');
      await doubleTap();
      expect(
        zoom.transformationController!.value.getMaxScaleOnAxis(),
        closeTo(1, 0.01),
      );

      await tester.tapAt(inFrame);
      await tester.pump(const Duration(milliseconds: 400));
      await pumpFrames(tester);
      final events = [for (final input in claktory.inputs) ...input.events];
      expect(events.map((event) => event.pointer.kind), [
        AgentBrowserPointerKind.AGENT_BROWSER_POINTER_KIND_DOWN,
        AgentBrowserPointerKind.AGENT_BROWSER_POINTER_KIND_UP,
      ]);
      expect(events.first.pointer.x, closeTo(0.5, 0.01));
      expect(events.first.pointer.y, closeTo(0.5, 0.01));
    } finally {
      reset();
    }
  });

  testWidgets("an iPad's check view keeps its band below the status bar and "
      "beside a window's controls", (tester) async {
    try {
      addTearDown(() => windowControls.value = WindowControls.none);
      tester.view.padding = const FakeViewPadding(top: 24);
      addTearDown(tester.view.resetPadding);
      await open(tester, _Device.ipad);
      await tester.tap(find.text('Solve check').last);
      await pumpFrames(tester);
      final back = find.descendant(
        of: find.byType(CheckView),
        matching: find.text('Back to Yukako'),
      );
      expect(tester.getRect(back).top, greaterThanOrEqualTo(24));

      // Windowed: the band goes beside the controls, below the status bar
      // only.
      tester.view.padding = const FakeViewPadding(top: 24 + 43);
      windowControls.value = const WindowControls(leading: 70, top: 43);
      await pumpFrames(tester);
      expect(tester.getRect(back).top, inInclusiveRange(24, 24 + 30));
      expect(tester.getRect(back).left, greaterThan(70));
    } finally {
      reset();
    }
  });

  for (final device in [_Device.mac, _Device.iphone]) {
    testWidgets('a check answered on another device closes its view here, '
        '${device.name}', (tester) async {
      try {
        final (harness, claktory) = await open(tester, device);
        await tester.tap(find.text('Solve check').last);
        await pumpFrames(tester);
        await claktory.frame(tester, request: 4);
        // Shown on the other device since: Claktory tells only that one
        // when it's answered there.
        await claktory.end(tester, AgentTabViewEnd.AGENT_TAB_VIEW_END_MOVED);
        expect(find.byType(CheckView), findsOneWidget);
        final answered = harness.backend.workspace.workspace.deepCopy();
        answered.agents.single.waitingSeqs.clear();
        await harness.apply(tester, answered);
        await pumpFrames(tester);
        expect(find.byType(CheckView), findsNothing);
        expect(
          find.text('Message Yukako'),
          findsOneWidget,
          reason: 'back at the conversation, once',
        );
      } finally {
        reset();
      }
    });
  }

  testWidgets("an iPad's Type into the page stays above the keyboard", (
    tester,
  ) async {
    try {
      await open(tester, _Device.ipad);
      await tester.tap(find.text('Solve check').last);
      await pumpFrames(tester);
      tester.view.viewInsets = const FakeViewPadding(bottom: 320);
      addTearDown(tester.view.resetViewInsets);
      await pumpFrames(tester);
      final field = find.text('Type into the page');
      expect(field, findsOneWidget);
      expect(tester.getRect(field).bottom, lessThanOrEqualTo(820 - 320));
    } finally {
      reset();
    }
  });

  for (final device in [_Device.mac, _Device.iphone]) {
    testWidgets("Claktory's note on input the check didn't take shows over "
        'the page for a moment, ${device.name}', (tester) async {
      try {
        final (_, claktory) = await open(tester, device);
        await tester.tap(find.text('Solve check').last);
        await pumpFrames(tester);
        await claktory.frame(tester, request: 4);
        const words = 'Only the check takes your clicks here.';
        await claktory.note(tester, words);
        expect(find.text(words), findsOneWidget);
        await _shot(tester, 'check-note-${device.name}');
        await tester.pump(AgentTabView.noteFor);
        await pumpFrames(tester);
        expect(find.text(words), findsNothing);
      } finally {
        reset();
      }
    });
  }

  for (final device in [_Device.mac, _Device.ipad]) {
    testWidgets('filling the window, the check view hides the conversation '
        'under it from VoiceOver, ${device.name}', (tester) async {
      final semantics = tester.ensureSemantics();
      try {
        final (_, claktory) = await open(tester, device);
        final covered = find.semantics.byLabel('More options for Yukako');
        expect(covered, findsOne);
        await tester.tap(find.text('Solve check').last);
        await pumpFrames(tester);
        await claktory.frame(tester, request: 4);
        expect(covered, findsNothing);
        expect(find.semantics.byLabel('Back to Yukako'), findsOne);
        expect(find.semantics.byLabel('The page on x.com, live'), findsOne);
        if (device == _Device.ipad) {
          expect(
            find.semantics.byPredicate(
              (node) =>
                  node.getSemanticsData().hint == 'Type into the page' ||
                  node.label == 'Type into the page',
            ),
            findsOne,
          );
        }
        await tester.sendKeyEvent(LogicalKeyboardKey.escape);
        await pumpFrames(tester);
        expect(covered, findsOne);
      } finally {
        semantics.dispose();
        reset();
      }
    });
  }
}
