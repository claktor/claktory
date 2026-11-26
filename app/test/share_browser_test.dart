// W6 on iPhone and iPad (#505): a friend's shared website opens in Safari's
// view over the app (ios/Runner/ShareBrowser.swift). What the app knows of
// it: which share's page is on screen, from which Claktory, and whether its
// first load failed; and which shares a phone keeps forwarded.
import 'package:factory_app/backend.dart' show sharesToForward;
import 'package:factory_app/mobile/share_browser.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  final studio = Object();
  final other = Object();
  late InAppShareBrowser page;

  setUp(() => page = InAppShareBrowser());
  tearDown(() => page.dispose());

  test('a page shows from the Claktory and share it was opened for, '
      'until Done', () {
    expect(page.shows(studio, 's1'), isFalse);
    page.opening(studio, 's1');
    // Not yet on screen: only once the native side says so.
    expect(page.shows(studio, 's1'), isFalse);
    page.debugEvent({'visible': true});
    expect(page.shows(studio, 's1'), isTrue);
    expect(page.showsFrom(studio), isTrue);
    expect(page.shows(studio, 's2'), isFalse);
    expect(page.shows(other, 's1'), isFalse);
    page.debugEvent({'visible': false});
    expect(page.shows(studio, 's1'), isFalse);
  });

  test("a first load that failed is said once, after Done, for that share's "
      'Claktory only', () {
    page
      ..opening(studio, 's1')
      ..debugEvent({'visible': true})
      ..debugEvent({'visible': true, 'loaded': false});
    // Still on screen: nothing to say yet.
    expect(page.takeFailure(studio, 's1'), isFalse);
    page.debugEvent({'visible': false});
    expect(page.takeFailure(other, 's1'), isFalse);
    expect(page.takeFailure(studio, 's2'), isFalse);
    expect(page.takeFailure(studio, 's1'), isTrue);
    expect(page.takeFailure(studio, 's1'), isFalse, reason: 'said once');
  });

  test('a page that loaded says nothing after Done', () {
    page
      ..opening(studio, 's1')
      ..debugEvent({'visible': true})
      ..debugEvent({'visible': false});
    expect(page.takeFailure(studio, 's1'), isFalse);
  });

  test('a page that never opened, or a removed Claktory, is forgotten', () {
    page
      ..opening(studio, 's1')
      ..openFailed(studio, 's1')
      ..debugEvent({'visible': true});
    expect(page.shows(studio, 's1'), isFalse);
    page
      ..opening(studio, 's2')
      ..debugEvent({'visible': true});
    expect(page.shows(studio, 's2'), isTrue);
    page.forget(studio);
    expect(page.showsFrom(studio), isFalse);
  });

  group('the shares a device forwards', () {
    PipeShare share(String id, {int port = 3000, bool listening = true}) =>
        PipeShare(
          id: id,
          address: 'http://s-$id.localhost:$port/',
          live: true,
          listening: listening,
        );
    final workspace = Workspace(
      pipeShares: [
        share('a'),
        share('b', port: 5173),
        share('c', listening: false),
        PipeShare(id: 'm', mine: true, address: 'http://x:1/', live: true),
      ],
    );

    test('a computer: every one opened there, live and not hidden', () {
      expect(
        sharesToForward(
          workspace,
          touch: false,
          forwarded: (_) => false,
          shown: (_) => false,
        ),
        {'a': 3000, 'b': 5173},
      );
    });

    test('a phone: only those it forwards already, and the one whose page '
        'is open over the app, again after a fresh tunnel', () {
      expect(
        sharesToForward(
          workspace,
          touch: true,
          forwarded: (_) => false,
          shown: (_) => false,
        ),
        isEmpty,
      );
      expect(
        sharesToForward(
          workspace,
          touch: true,
          forwarded: (id) => id == 'b',
          shown: (_) => false,
        ),
        {'b': 5173},
      );
      // The tunnel was made afresh (back from the Home Screen): nothing is
      // forwarded any more, but the page over the app is again.
      expect(
        sharesToForward(
          workspace,
          touch: true,
          forwarded: (_) => false,
          shown: (id) => id == 'a',
        ),
        {'a': 3000},
      );
    });
  });
}
