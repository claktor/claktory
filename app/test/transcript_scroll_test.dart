import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/transcript.dart';
import 'package:factory_app/waiting.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:flutter_test/flutter_test.dart';

const _width = 900.0;
const _height = 600.0;

/// An agent's reply of a few lines, more for some, so entries differ in
/// height as real ones do.
Entry _reply(int seq) => Entry(
  seq: Int64(seq),
  assistant: AssistantText(
    text: [
      'reply $seq',
      for (var line = 0; line < (seq * 7) % 11; line++) 'line $line of $seq',
    ].join('\n\n'),
  ),
);

TranscriptStore _store(int from, int to, {bool hasEarlier = false}) =>
    TranscriptStore()..load(
      Transcript(
        agentId: 'a',
        entries: [for (var i = from; i < to; i++) _reply(i)],
        hasEarlier: hasEarlier,
      ),
    );

Future<void> _show(
  WidgetTester tester,
  TranscriptStore store, {
  Listenable? follow,
  EntryReveal? reveal,
  Future<void> Function()? loadEarlier,
  Future<void> Function(int seq)? loadFrom,
  Size size = const Size(_width, _height),
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    WidgetsApp(
      color: const Color(0xFF000000),
      builder: (context, _) => Overlay(
        initialEntries: [
          OverlayEntry(
            builder: (context) => TranscriptView(
              key: ValueKey(store.loads),
              transcript: store,
              working: false,
              follow: follow ?? ValueNotifier(0),
              agentId: 'a',
              reveal: reveal,
              loadEarlier: loadEarlier,
              loadFrom: loadFrom,
            ),
          ),
        ],
      ),
    ),
  );
  await tester.pumpAndSettle();
}

Finder _entry(int seq) => find.byWidgetPredicate(
  (widget) => widget is EntryView && widget.model.seq == seq,
);

/// The bottom of entry [seq], on screen.
double _bottomOf(WidgetTester tester, int seq) =>
    tester.getRect(_entry(seq)).bottom;

ScrollPosition _position(WidgetTester tester) =>
    tester.state<ScrollableState>(find.byType(Scrollable)).position;

/// Scrolls all the way down, as the person does with the wheel.
Future<void> _toBottom(WidgetTester tester) async {
  for (var i = 0; i < 400; i++) {
    final position = _position(tester);
    if (position.pixels >= position.maxScrollExtent) break;
    await tester.drag(find.byType(CustomScrollView), const Offset(0, -500));
    await tester.pump();
  }
  await tester.pumpAndSettle();
}

/// Adds [count] replies after [last], frame by frame, checking at every
/// frame that the newest shown sits at the bottom of the view.
Future<int> _stream(
  WidgetTester tester,
  TranscriptStore store,
  int last,
  int count,
) async {
  for (var seq = last + 1; seq <= last + count; seq++) {
    store.update(EntryUpdate(agentId: 'a', entry: _reply(seq)));
    for (var frame = 0; frame < 25; frame++) {
      await tester.pump(const Duration(milliseconds: 16));
      expect(
        _entry(seq - 1),
        findsOneWidget,
        reason: 'the entry before $seq is built at frame $frame',
      );
      final bottom = tester.getRect(_entry(seq - 1)).bottom;
      expect(
        bottom,
        lessThanOrEqualTo(_height + 1),
        reason: 'reply ${seq - 1} not pushed below at frame $frame',
      );
    }
    final bottom = _bottomOf(tester, seq);
    expect(bottom, lessThanOrEqualTo(_height), reason: 'reply $seq in view');
    expect(
      bottom,
      greaterThan(_height - 60),
      reason: 'reply $seq at the bottom, not above it',
    );
  }
  return last + count;
}

void main() {
  test('the conversation sent again is taken in place, keeping what was '
      'paged in; a cleared one loads afresh', () {
    final store = _store(100, 200, hasEarlier: true);
    store.prepend('a', [
      for (var i = 50; i < 100; i++) _reply(i),
    ], hasEarlier: true);
    final loads = store.loads;
    final page = Transcript(
      agentId: 'a',
      entries: [for (var i = 150; i < 202; i++) _reply(i)],
      hasEarlier: true,
    );
    page.entries[0] = page.entries[0].deepCopy()
      ..assistant.text = 'reply 150, changed';
    store.load(page);
    expect(store.loads, loads);
    expect(store.length, 152);
    expect(store[0].seq, 50);
    expect(store[store.length - 1].seq, 201);
    expect(store[100].entry.assistant.text, 'reply 150, changed');

    store.load(
      Transcript(
        agentId: 'a',
        entries: [
          Entry(
            seq: Int64(0),
            timestampMs: Int64(9),
            notice: Notice(text: 'cleared'),
          ),
        ],
      ),
    );
    expect(store.loads, loads + 1);
    expect(store.length, 1);
  });

  testWidgets('a long transcript builds only what shows', (tester) async {
    final store = _store(0, 2000);
    await _show(tester, store);
    final built = find.byType(EntryView).evaluate().length;
    expect(built, lessThan(40));
    expect(_bottomOf(tester, 1999), lessThanOrEqualTo(_height));
  });

  testWidgets('streaming while at the bottom stays at the bottom', (
    tester,
  ) async {
    final store = _store(0, 300);
    await _show(tester, store);
    await _stream(tester, store, 299, 30);
  });

  testWidgets('opened at a waiting card far back, then scrolled down: new '
      'replies are followed at the bottom without jumping', (tester) async {
    final store = _store(0, 300);
    final reveal = EntryReveal()..value = (agentId: 'a', seq: 5);
    await _show(tester, store, reveal: reveal);
    expect(
      tester.getRect(find.textContaining('reply 5')).top,
      lessThan(100),
      reason: 'the card at the top',
    );
    await _toBottom(tester);
    expect(_bottomOf(tester, 299), lessThanOrEqualTo(_height));
    await _stream(tester, store, 299, 30);
  });

  testWidgets('scrolling down from a card far back moves as far as the '
      'person scrolls, all the way to the end', (tester) async {
    final store = _store(0, 1500);
    final reveal = EntryReveal()..value = (agentId: 'a', seq: 3);
    await _show(tester, store, reveal: reveal);
    var last = _position(tester).pixels;
    for (var i = 0; i < 600; i++) {
      final position = _position(tester);
      if (position.pixels >= position.maxScrollExtent &&
          find
              .byWidgetPredicate((w) => w is EntryView && w.model.seq == 1499)
              .evaluate()
              .isNotEmpty) {
        break;
      }
      await tester.drag(find.byType(CustomScrollView), const Offset(0, -1000));
      await tester.pump();
      await tester.pump();
      final moved = _position(tester).pixels - last;
      expect(moved, lessThan(1100), reason: 'no leap at step $i');
      last = _position(tester).pixels;
    }
    expect(_bottomOf(tester, 1499), lessThanOrEqualTo(_height));
    await _stream(tester, store, 1499, 10);
  });

  testWidgets('earlier pages loaded while scrolled up, then back down: new '
      'replies are followed', (tester) async {
    final store = _store(1000, 1040, hasEarlier: true);
    var next = 1000;
    await _show(
      tester,
      store,
      loadEarlier: () async {
        final from = next - 40;
        store.prepend('a', [
          for (var i = from; i < next; i++) _reply(i),
        ], hasEarlier: from > 0);
        next = from;
      },
    );
    for (var i = 0; i < 30; i++) {
      await tester.drag(find.byType(CustomScrollView), const Offset(0, 800));
      await tester.pump();
      await tester.pump();
    }
    expect(next, lessThan(1000), reason: 'pages came in');
    await _toBottom(tester);
    await _stream(tester, store, 1039, 20);
  });

  testWidgets('a resized window keeps the bottom while following', (
    tester,
  ) async {
    final store = _store(0, 300);
    await _show(tester, store);
    for (final width in [700.0, 1200.0, 500.0, 900.0]) {
      tester.view.physicalSize = Size(width, _height);
      await tester.pump();
      await tester.pumpAndSettle();
      expect(_bottomOf(tester, 299), lessThanOrEqualTo(_height));
      expect(_bottomOf(tester, 299), greaterThan(_height - 60));
    }
    await _stream(tester, store, 299, 10);
  });

  // A Mac window keeps its composition at 600; an Android window this
  // narrow takes the phone's layout instead.
  testWidgets('scrolled up, a resize keeps what was shown on screen', (
    tester,
  ) async {
    final store = _store(0, 300);
    await _show(tester, store);
    for (var i = 0; i < 10; i++) {
      await tester.drag(find.byType(CustomScrollView), const Offset(0, 500));
      await tester.pump();
    }
    await tester.pumpAndSettle();
    final pixels = _position(tester).pixels;
    tester.view.physicalSize = const Size(600, _height);
    await tester.pumpAndSettle();
    // Not thrown back to the start or the end.
    final position = _position(tester);
    expect(position.pixels, greaterThan(position.minScrollExtent + 1000));
    expect(position.pixels, lessThan(position.maxScrollExtent - 1000));
    expect(pixels, isNot(position.minScrollExtent));
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('scrolled up, a button at the bottom right goes back down, '
      'saying how many replies came in; there, following resumes', (
    tester,
  ) async {
    final store = _store(0, 300);
    await _show(tester, store);
    bool shown() => !tester
        .widget<IgnorePointer>(
          find
              .ancestor(
                of: find.byKey(const ValueKey('latest')),
                matching: find.byType(IgnorePointer),
              )
              .first,
        )
        .ignoring;
    // At the end: none.
    expect(shown(), isFalse);

    await tester.drag(find.byType(CustomScrollView), const Offset(0, 3000));
    await tester.pumpAndSettle();
    expect(shown(), isTrue);
    expect(find.textContaining('new'), findsNothing);
    final button = tester.getRect(find.byKey(const ValueKey('latest')));
    expect(button.height, 28, reason: "a pointer's, on a Mac");
    expect(button.right, closeTo(_width - 16, 1));
    expect(button.bottom, closeTo(_height - 12, 1));

    // Replies stream in meanwhile: counted, and the view stays put.
    final top = _position(tester).pixels;
    for (var seq = 300; seq < 303; seq++) {
      store.update(EntryUpdate(agentId: 'a', entry: _reply(seq)));
      await tester.pump(const Duration(milliseconds: 16));
    }
    await tester.pumpAndSettle();
    expect(find.text('3 new'), findsOneWidget);
    expect(_position(tester).pixels, top);

    await tester.tap(find.byKey(const ValueKey('latest')));
    for (var frame = 0; frame < 120; frame++) {
      await tester.pump(const Duration(milliseconds: 16));
    }
    await tester.pumpAndSettle();
    expect(_bottomOf(tester, 302), lessThanOrEqualTo(_height));
    expect(_bottomOf(tester, 302), greaterThan(_height - 60));
    expect(shown(), isFalse);
    // Following again: what streams in now stays at the bottom.
    await _stream(tester, store, 302, 10);
  });

  testWidgets("on Android it's a finger's 48, wide or on a phone", (
    tester,
  ) async {
    PhonePlatformPolicy.debugAndroidPolicy = true;
    addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
    for (final size in const [Size(1200, 800), Size(400, 800)]) {
      final semantics = tester.ensureSemantics();
      await _show(tester, _store(0, 300), size: size);
      await tester.drag(find.byType(CustomScrollView), const Offset(0, 3000));
      await tester.pumpAndSettle();
      final button = tester.getRect(
        find.bySemanticsLabel('Go to the latest message'),
      );
      expect(button.height, greaterThanOrEqualTo(48), reason: '$size');
      expect(button.width, greaterThanOrEqualTo(48), reason: '$size');
      semantics.dispose();
    }
  });
}
