import 'dart:convert';

import 'package:factory_app/controls.dart';
import 'package:factory_app/explorer.dart';
import 'package:factory_app/panes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

ViewNode leaf(String id, String kind, [String path = '']) => ViewNode(
  id: id,
  leaf: ViewLeaf(kind: kind, path: path, title: path.split('/').last),
);

ViewNode split(
  String id,
  ViewAxis axis,
  List<ViewNode> children, [
  List<double> weights = const [],
]) => ViewNode(
  id: id,
  split: ViewSplit(axis: axis, children: children, weights: weights),
);

const row = ViewAxis.VIEW_AXIS_ROW;
const column = ViewAxis.VIEW_AXIS_COLUMN;

/// The conversation with main.rs beside it.
ViewNode beside([String kind = 'text', String path = '/w/main.rs']) =>
    split('s1', row, [leaf('chat', 'chat'), leaf('v1', kind, path)]);

Workspace snapshot({ViewNode? layout, String focused = ''}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/w')],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
    Agent(
      id: 'a1',
      projectId: 'p1',
      name: 'Builder',
      modelAvailable: true,
      layout: layout,
    ),
  ],
  selectedAgentId: 'a1',
  focusedProjectId: focused,
  highlights: true,
);

FileContent text(String path, String body, {String mime = 'text/plain'}) =>
    FileContent(
      path: path,
      mime: mime,
      size: Int64(utf8.encode(body).length),
      data: utf8.encode(body),
    );

Future<Harness> showPanes(
  WidgetTester tester,
  ViewNode? layout, {
  Map<String, FileContent> files = const {},
  String focused = '',
  Explorer? explorer,
}) async {
  final harness = Harness(snapshot(layout: layout, focused: focused));
  harness.backend.files.addAll(files);
  if (explorer != null) harness.backend.explorer.apply(explorer);
  await harness.show(tester);
  await settle(tester);
  return harness;
}

Finder glyphButton(String label) => find.byWidgetPredicate(
  (widget) => widget is GlyphButton && widget.label == label,
);

List<ClientMessage> layoutsSent(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasSetLayout()) message,
];

int reads(Harness harness) =>
    harness.backend.sent.where((message) => message.hasReadFile()).length;

void main() {
  testWidgets('with only the conversation there is no pane chrome', (
    tester,
  ) async {
    await showPanes(tester, null);
    expect(find.byType(Pane), findsNothing);
    expect(find.text('Conversation'), findsNothing);
  });

  testWidgets('a text viewer shows the file with line numbers beside the '
      'conversation', (tester) async {
    final harness = await showPanes(
      tester,
      beside(),
      files: {'/w/main.rs': text('/w/main.rs', 'fn main() {\n}\n')},
    );
    expect(find.byType(Pane), findsNWidgets(2));
    expect(find.text('Conversation'), findsOneWidget);
    expect(find.text('main.rs'), findsOneWidget);
    expect(find.text('14 bytes'), findsOneWidget, reason: "the file's size");
    expect(find.text('fn main() {'), findsOneWidget);
    expect(find.text('1'), findsOneWidget);
    expect(find.text('2'), findsOneWidget);
    expect(harness.last.readFile.path, '/w/main.rs');
    // Side by side, sharing the room equally.
    final chat = tester.getRect(find.byKey(const ValueKey('pane-chat')));
    final viewer = tester.getRect(find.byKey(const ValueKey('pane-v1')));
    expect(viewer.left, greaterThan(chat.right));
    expect((chat.width - viewer.width).abs(), lessThanOrEqualTo(1));
  });

  // The viewers themselves are tested in viewers_test.dart.

  testWidgets('closing a viewer sends the layout without it', (tester) async {
    final harness = await showPanes(tester, beside());
    await tester.tap(glyphButton('Close'));
    await settle(tester);
    final sent = harness.last.setLayout;
    expect(sent.agentId, 'a1');
    expect(sent.hasRoot(), isFalse, reason: 'only the conversation is left');
  });

  testWidgets('dragging a pane by its header onto an edge moves it there', (
    tester,
  ) async {
    final harness = await showPanes(tester, beside());
    final chat = tester.getRect(find.byKey(const ValueKey('pane-chat')));
    final gesture = await tester.startGesture(
      tester.getCenter(find.text('main.rs')),
    );
    await tester.pump();
    await gesture.moveBy(const Offset(-40, 0));
    await tester.pump();
    // Over the conversation's left edge: its left half lights up.
    await gesture.moveTo(Offset(chat.left + 30, chat.center.dy));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 200));
    final preview = tester.widget<DropPreview>(
      find.descendant(
        of: find.byKey(const ValueKey('pane-chat')),
        matching: find.byType(DropPreview),
      ),
    );
    expect(preview.shown, isTrue);
    expect(preview.edge?.name, 'left');
    // The middle third takes nothing.
    await gesture.moveTo(chat.center);
    await tester.pump();
    expect(
      tester
          .widget<DropPreview>(
            find.descendant(
              of: find.byKey(const ValueKey('pane-chat')),
              matching: find.byType(DropPreview),
            ),
          )
          .shown,
      isFalse,
    );
    await gesture.moveTo(Offset(chat.left + 30, chat.center.dy));
    await tester.pump();
    await gesture.up();
    await settle(tester);
    final root = harness.last.setLayout.root;
    expect(root.split.axis, row);
    expect([for (final child in root.split.children) child.id], ['v1', 'chat']);
  });

  testWidgets('a divider resizes its split, sends it once, and a double '
      'click shares it equally again', (tester) async {
    final harness = await showPanes(tester, beside());
    final divider = find.byKey(const ValueKey('divider-s1-0'));
    final before = tester.getRect(find.byKey(const ValueKey('pane-chat')));
    final gesture = await tester.startGesture(tester.getCenter(divider));
    for (var i = 0; i < 10; i++) {
      await gesture.moveBy(const Offset(10, 0));
      await tester.pump();
    }
    // Live while dragged, and nothing sent yet.
    expect(
      tester.getRect(find.byKey(const ValueKey('pane-chat'))).width,
      greaterThan(before.width + 50),
    );
    expect(layoutsSent(harness), isEmpty);
    // Past the other pane's smallest size: held there.
    await gesture.moveBy(const Offset(2000, 0));
    await tester.pump();
    final viewer = tester.getRect(find.byKey(const ValueKey('pane-v1')));
    expect(viewer.width, closeTo(paneMinWidth, 1));
    await gesture.up();
    await settle(tester);
    expect(layoutsSent(harness), hasLength(1));
    final weights = harness.last.setLayout.root.split.weights;
    expect(weights, hasLength(2));
    expect(weights.first, greaterThan(weights.last));

    await tester.tap(divider);
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tap(divider);
    await settle(tester);
    expect(layoutsSent(harness), hasLength(2));
    expect(harness.last.setLayout.root.split.weights, isEmpty);
  });

  testWidgets('a file already open glows rather than opening again, and '
      "an agent's showing it again reads it again", (tester) async {
    final harness = await showPanes(
      tester,
      split('s1', row, [
        leaf('chat', 'chat'),
        leaf('f1', 'files', 'p1'),
        leaf('v1', 'text', '/w/main.rs'),
      ]),
      explorer: Explorer(
        projectId: 'p1',
        entries: [FileEntry(path: 'main.rs', name: 'main.rs')],
      ),
    );
    expect(reads(harness), 1);
    await tester.tap(
      find.descendant(
        of: find.byType(FilesPane),
        matching: find.text('main.rs'),
      ),
    );
    await settle(tester);
    expect(layoutsSent(harness), isEmpty);
    expect(
      harness.backend.highlights.glows.map((glow) => glow.key),
      contains('pane:a1:v1'),
    );

    harness.backend.highlights.apply(
      Highlight(
        kind: HighlightKind.HIGHLIGHT_KIND_AGENT,
        targets: [HighlightTarget(kind: 'pane', id: 'a1:v1')],
      ),
    );
    await settle(tester);
    expect(reads(harness), 2);
    await tester.pump(const Duration(seconds: 2));
  });

  testWidgets("a phone lists the agent's viewers and pushes one", (
    tester,
  ) async {
    final harness = Harness(snapshot(layout: beside()));
    harness.backend.files['/w/main.rs'] = text('/w/main.rs', 'fn main() {}');
    await harness.show(tester, size: const Size(390, 844));
    await pumpFrames(tester);
    await tester.tap(find.text('Builder'));
    await pumpFrames(tester);
    expect(find.byType(Pane), findsNothing, reason: 'one pane at a time');
    await tester.tap(find.bySemanticsLabel('Extensions'));
    await pumpFrames(tester);
    expect(find.text('Files'), findsOneWidget);
    await tester.tap(find.text('main.rs'));
    await pumpFrames(tester);
    expect(find.text('fn main() {}'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(find.text('fn main() {}'), findsNothing);
  });
}
