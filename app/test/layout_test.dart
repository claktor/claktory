import 'package:factory_app/layout.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter_test/flutter_test.dart';

ViewNode leaf(String id, [String kind = 'text']) => ViewNode(
  id: id,
  leaf: ViewLeaf(kind: kind, path: kind == chatKind ? '' : '/f/$id'),
);

ViewNode row(String id, List<ViewNode> children, [List<double>? weights]) =>
    ViewNode(
      id: id,
      split: ViewSplit(
        axis: ViewAxis.VIEW_AXIS_ROW,
        children: children,
        weights: weights ?? const [],
      ),
    );

ViewNode column(String id, List<ViewNode> children) => ViewNode(
  id: id,
  split: ViewSplit(axis: ViewAxis.VIEW_AXIS_COLUMN, children: children),
);

/// The tree as text: leaves by id, splits as row(…) or col(…).
String shape(ViewNode node) => node.hasSplit()
    ? '${node.split.axis == ViewAxis.VIEW_AXIS_ROW ? 'row' : 'col'}'
          '(${node.split.children.map(shape).join(' ')})'
    : node.id;

void main() {
  final chat = leaf('chat', chatKind);

  test('no layout, or one without the conversation, is just the chat', () {
    expect(isChatOnly(layoutOf(Agent())), isTrue);
    expect(
      isChatOnly(layoutOf(Agent(layout: row('r', [leaf('a'), leaf('b')])))),
      isTrue,
    );
  });

  test('dropping on an edge splits that pane equally', () {
    final root = row('r', [chat, leaf('a')]);
    expect(
      shape(insert(root, leaf('b'), 'a', DropEdge.bottom)!),
      'row(chat col(a b))',
    );
    expect(
      shape(insert(root, leaf('b'), 'a', DropEdge.left)!),
      'row(chat row(b a))',
      reason: 'the dropped pane and its target share its half',
    );
    expect(insert(root, leaf('b'), 'gone', DropEdge.top), isNull);
  });

  test('moving a pane, the conversation too', () {
    final root = row('r', [
      chat,
      column('c', [leaf('a'), leaf('b')]),
    ]);
    expect(shape(move(root, 'b', 'chat', DropEdge.top)!), 'row(col(b chat) a)');
    expect(
      shape(move(root, 'chat', 'b', DropEdge.right)!),
      'col(a row(b chat))',
    );
    expect(move(root, 'a', 'a', DropEdge.top), isNull);
  });

  test('closing a viewer folds away its split; the chat stays', () {
    final root = row('r', [
      chat,
      column('c', [leaf('a'), leaf('b')]),
    ]);
    expect(shape(close(root, 'a')!), 'row(chat b)');
    expect(shape(close(close(root, 'a')!, 'b')!), 'chat');
    expect(close(root, 'chat'), isNull);
  });

  test('closing keeps the others\' shares', () {
    final root = row('r', [chat, leaf('a'), leaf('b')], [2, 1, 1]);
    final closed = close(root, 'b')!;
    expect(shape(closed), 'row(chat a)');
    expect(sharesOf(closed.split), [2 / 3, 1 / 3]);
  });

  test('opening a file: beside the chat, then stacked, then the oldest '
      'goes', () {
    var root = chatOnly();
    final first = open(root, '/p/a.md');
    root = first.root;
    expect(leaves(root).map((leaf) => leaf.leaf.kind), [chatKind, 'text']);
    expect(root.split.axis, ViewAxis.VIEW_AXIS_ROW);

    root = open(root, '/p/b.png').root;
    root = open(root, '/p/c.txt').root;
    final viewers = root.split.children[1];
    expect(viewers.split.axis, ViewAxis.VIEW_AXIS_COLUMN);
    expect(viewers.split.children, hasLength(3), reason: 'one column');
    expect(viewers.split.children[1].leaf.kind, 'image');

    // Already open: nothing changes, and its pane is named.
    final again = open(root, '/p/a.md');
    expect(again.root, same(root));
    expect(again.paneId, first.paneId);

    for (var i = 0; i < 20; i++) {
      root = open(root, '/p/more$i.txt').root;
    }
    expect(leaves(root), hasLength(maxPanes));
    expect(leaves(root).any((leaf) => leaf.leaf.kind == chatKind), isTrue);
    expect(leaves(root).any((leaf) => leaf.leaf.path == '/p/a.md'), isFalse);
  });

  test('ids stay unique', () {
    var root = chatOnly();
    for (var i = 0; i < 11; i++) {
      root = open(root, '/p/$i').root;
    }
    final ids = <String>[];
    void walk(ViewNode node) {
      ids.add(node.id);
      if (node.hasSplit()) node.split.children.forEach(walk);
    }

    walk(root);
    expect(ids.toSet(), hasLength(ids.length));
  });

  test('resizing sets a split\'s shares', () {
    final root = row('r', [chat, leaf('a')]);
    expect(resize(root, 'r', [0.7, 0.3]).split.weights, [0.7, 0.3]);
  });
}
