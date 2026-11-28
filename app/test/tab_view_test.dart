// The person's live views of an agent's browser (errands/tab_view.dart):
// what each asks of Claktory, which frames it takes, and that only a
// check's tab sends input.
import 'dart:async';
import 'dart:ui' as ui;

import 'package:factory_app/errands/live_input.dart';
import 'package:factory_app/errands/tab_view.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

class _Claktory {
  _Claktory() {
    views = AgentTabViews(
      send: (message) async {
        sent.add(message);
        if (refuse case final why?) throw Exception(why);
      },
      messages: messages.stream,
      connected: connected,
      decode: (bytes) {
        final recorder = ui.PictureRecorder();
        ui.Canvas(recorder);
        return recorder.endRecording().toImage(128, 80);
      },
    );
  }

  final sent = <ClientMessage>[];
  final messages = StreamController<ServerMessage>.broadcast(sync: true);
  final connected = ValueNotifier(true);
  String? refuse;
  late final AgentTabViews views;

  List<WatchAgentTab> get watches => [
    for (final message in sent)
      if (message.hasWatchAgentTab()) message.watchAgentTab,
  ];

  List<AgentTabInput> get inputs => [
    for (final message in sent)
      if (message.hasAgentTabInput()) message.agentTabInput,
  ];

  void frame(String agent, int request, int seq, {String site = 'x.com'}) =>
      messages.add(
        ServerMessage(
          agentTabFrame: AgentTabFrame(
            agentId: agent,
            requestSeq: Int64(request),
            frameSeq: Int64(seq),
            jpeg: [1, 2, 3],
            width: 128,
            height: 80,
            site: site,
            title: 'Just a moment…',
          ),
        ),
      );
}

void main() {
  testWidgets("a watch takes only its own agent's frames, and acks each", (
    tester,
  ) async {
    final claktory = _Claktory();
    final view = claktory.views.open('yukako', maxWidth: 1600);
    addTearDown(view.dispose);
    expect(claktory.watches.single.agentId, 'yukako');
    expect(claktory.watches.single.requestSeq, Int64.ZERO);
    expect(claktory.watches.single.on, isTrue);
    expect(claktory.watches.single.maxWidth, 1600);

    await tester.runAsync(() async {
      claktory.frame('june', 0, 1, site: 'evil.example');
      claktory.frame('yukako', 7, 2, site: 'evil.example');
      claktory.frame('yukako', 0, 3);
      await Future<void>.delayed(const Duration(milliseconds: 20));
    });
    expect(view.frames.frame?.seq, 3);
    expect(view.site, 'x.com');
    final acks = [
      for (final message in claktory.sent)
        if (message.hasAgentTabFrameAck()) message.agentTabFrameAck.frameSeq,
    ];
    expect(acks, [Int64(3)]);
  });

  testWidgets('a watch sends no input; a check sends it, without the '
      'secondary button', (tester) async {
    final claktory = _Claktory();
    final watch = claktory.views.open('yukako');
    await tester.runAsync(() async {
      claktory.frame('yukako', 0, 1);
      await Future<void>.delayed(const Duration(milliseconds: 20));
    });
    watch.input(const [LivePointer(LivePointerKind.down, 0.5, 0.5)]);
    expect(claktory.inputs, isEmpty);

    final check = claktory.views.open('yukako', requestSeq: 42);
    await tester.runAsync(() async {
      claktory.frame('yukako', 42, 5);
      await Future<void>.delayed(const Duration(milliseconds: 20));
    });
    check.input(const [
      LivePointer(LivePointerKind.down, 0.25, 0.5, button: LiveButton.right),
      LivePointer(LivePointerKind.down, 0.25, 0.5),
      LiveTyped('abc'),
    ]);
    final input = claktory.inputs.single;
    expect(input.agentId, 'yukako');
    expect(input.requestSeq, Int64(42));
    expect(input.events.length, 2);
    expect(input.events.first.pointer.button, 1);
    expect(input.events.first.pointer.frameSeq, Int64(5));
    expect(input.events.last.text, 'abc');
    check.dispose();
    watch.dispose();
  });

  testWidgets('a check opened over a watch takes the connection; closed, '
      'the watch watches again', (tester) async {
    final claktory = _Claktory();
    final watch = claktory.views.open('yukako');
    final check = claktory.views.open('yukako', requestSeq: 42);
    expect(watch.watching, isFalse);
    expect(check.watching, isTrue);
    claktory.sent.clear();
    check.close();
    expect(claktory.watches.single.requestSeq, Int64.ZERO);
    expect(claktory.watches.single.on, isTrue);
    expect(watch.watching, isTrue);
    claktory.sent.clear();
    watch.close();
    expect(claktory.watches.single.on, isFalse);
    check.dispose();
    watch.dispose();
  });

  test('moved to another device it says where, and watches here again on '
      'asking', () {
    final claktory = _Claktory();
    final view = claktory.views.open('yukako');
    claktory.messages.add(
      ServerMessage(
        agentTabViewEnded: AgentTabViewEnded(
          agentId: 'yukako',
          reason: AgentTabViewEnd.AGENT_TAB_VIEW_END_MOVED,
          device: "Matt's iPhone",
        ),
      ),
    );
    expect(view.watching, isFalse);
    expect(view.ended?.device, "Matt's iPhone");
    claktory.sent.clear();
    view.again();
    expect(view.ended, isNull);
    expect(claktory.watches.single.on, isTrue);
    view.dispose();
  });

  test('back after a dropped connection, the view on top watches again', () {
    final claktory = _Claktory();
    final view = claktory.views.open('yukako', requestSeq: 42);
    claktory.connected.value = false;
    expect(view.watching, isFalse);
    claktory.sent.clear();
    claktory.connected.value = true;
    expect(claktory.watches.single.requestSeq, Int64(42));
    expect(view.watching, isTrue);
    view.dispose();
  });

  test("refused, it says Claktory's words", () async {
    final claktory = _Claktory()..refuse = "Yukako's browser isn't open.";
    final view = claktory.views.open('yukako');
    await Future<void>.delayed(Duration.zero);
    expect(view.watching, isFalse);
    expect(view.error, contains("Yukako's browser isn't open."));
    view.dispose();
  });

  testWidgets("Claktory's note is its view's alone, and goes after a moment", (
    tester,
  ) async {
    final claktory = _Claktory();
    final check = claktory.views.open('yukako', requestSeq: 42);
    addTearDown(check.dispose);
    void note(int request, String words) => claktory.messages.add(
      ServerMessage(
        agentTabNote: AgentTabNote(
          agentId: 'yukako',
          requestSeq: Int64(request),
          message: words,
        ),
      ),
    );
    note(7, 'Not this one.');
    expect(check.note, isNull);
    note(42, 'Only the check takes your clicks here.');
    expect(check.note, 'Only the check takes your clicks here.');
    await tester.pump(AgentTabView.noteFor - const Duration(milliseconds: 1));
    expect(check.note, isNotNull);
    await tester.pump(const Duration(milliseconds: 1));
    expect(check.note, isNull);
  });
}
