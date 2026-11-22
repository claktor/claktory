// PDFs on an iPhone (viewers/pdf.dart, as a page): PDFKit's iOS view with
// nothing drawing, so a finger scrolls, zooms and selects and the page's
// Back from the edge still works; "Page 2 of 9" in the header; Search in
// More opens a find bar at the page's foot, above the keyboard, with the
// previous and next match and Done; a link asks first.
import 'dart:convert';

import 'package:crypto/crypto.dart';
import 'package:factory_app/ipad/ink_bridge.dart';
import 'package:factory_app/mobile/find_bar.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/pdf.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, snapshot;

const _path = '/w/paper.pdf';

FileContent _pdf() {
  final bytes = utf8.encode('%PDF-1.7\na paper about lapwings');
  return FileContent(
    path: _path,
    mime: 'application/pdf',
    size: Int64(bytes.length),
    data: bytes,
    sha256: sha256.convert(bytes).toString(),
  );
}

TestDefaultBinaryMessenger get _messenger =>
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

/// The render cache and the native view: what each was asked.
class _Native {
  _Native() {
    InkRenderCache.channel = const MethodChannel('test/phone-pdf-cache');
    _messenger.setMockMethodCallHandler(InkRenderCache.channel, (call) async {
      final args = (call.arguments as Map?)?.cast<String, Object?>() ?? {};
      return switch (call.method) {
        'cacheLookup' => <String, Object?>{'file': null},
        'cacheBegin' => () {
          _sha = args['sha256'] as String;
          return {'token': 't'};
        }(),
        'cacheFinish' => {'file': '/cache/$_sha/paper.pdf'},
        _ => null,
      };
    });
    _messenger.setMockMethodCallHandler(SystemChannels.platform_views, (
      call,
    ) async {
      if (call.method != 'create') return null;
      final args = call.arguments as Map<Object?, Object?>;
      made.add(args);
      final id = args['id'] as int;
      _messenger.setMockMethodCallHandler(MethodChannel('claktory/ink/$id'), (
        call,
      ) async {
        calls.add(call);
        final a = (call.arguments as Map?)?.cast<String, Object?>() ?? {};
        switch (call.method) {
          case 'search':
            final query = a['query'] as String;
            return {
              'query': query,
              'matches': query.isEmpty
                  ? []
                  : [
                      for (final page in [2, 5, 7])
                        {
                          'page_1based': page,
                          'region': {'x': 0.1, 'y': 0.2, 'w': 0.3, 'h': 0.04},
                          'text': 'the $query here',
                        },
                    ],
              'truncated': false,
            };
          case 'goToPage':
            return {'page_1based': a['page_1based']};
        }
        return null;
      });
      return null;
    });
    addTearDown(() {
      _messenger.setMockMethodCallHandler(InkRenderCache.channel, null);
      InkRenderCache.channel = const MethodChannel('claktory/ink');
      _messenger.setMockMethodCallHandler(SystemChannels.platform_views, null);
      for (final view in made) {
        _messenger.setMockMethodCallHandler(
          MethodChannel('claktory/ink/${view['id']}'),
          null,
        );
      }
    });
  }

  var _sha = '';
  final made = <Map<Object?, Object?>>[];
  final calls = <MethodCall>[];

  Map<Object?, Object?> get params =>
      const StandardMessageCodec().decodeMessage(
        ByteData.sublistView(made.last['params'] as Uint8List),
      ) as Map<Object?, Object?>;

  Future<void> send(String method, Object? arguments) async {
    final id = made.last['id'] as int;
    await _messenger.handlePlatformMessage(
      'claktory/ink/$id',
      const StandardMethodCodec().encodeMethodCall(
        MethodCall(method, arguments),
      ),
      (_) {},
    );
  }

  Iterable<Map<String, Object?>> pagesGone() => [
    for (final call in calls)
      if (call.method == 'goToPage')
        (call.arguments as Map).cast<String, Object?>(),
  ];
}

/// An iPhone with the PDF pushed over the agent's conversation.
Future<Harness> _phone(
  WidgetTester tester, {
  Size size = const Size(393, 852),
  double textScale = 1,
}) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  tester.platformDispatcher.textScaleFactorTestValue = textScale;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  final harness = Harness(snapshot(layout: beside('pdf', _path)));
  harness.backend.files[_path] = _pdf();
  harness.backend.rangedReads = true;
  tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
  addTearDown(tester.view.resetPadding);
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  await tester.tap(find.bySemanticsLabel('Extensions'));
  await pumpFrames(tester);
  await tester.tap(find.text('paper.pdf').last);
  for (var i = 0; i < 4; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 20)),
    );
    await tester.pump();
  }
  await pumpFrames(tester);
  return harness;
}

Future<void> _openSearch(WidgetTester tester) async {
  await tester.tap(find.bySemanticsLabel('More').last);
  await pumpFrames(tester);
  await tester.tap(find.text('Search').last);
  await pumpFrames(tester);
}

/// Scratch shots (a zz test sets it).
Future<void> Function(WidgetTester, String)? shots;

void main() {
  testWidgets('a PDF opens as a page in the iOS view, only to read, its '
      'page in the header', (tester) async {
    final native = _Native();
    await _phone(tester);
    expect(find.byType(PdfViewer), findsOneWidget);
    expect(find.byType(UiKitView), findsOneWidget);
    expect(native.params['surface'], 'pdf');
    expect(native.params['drawing_policy'], 'none');
    // Nothing claims a touch first: Back from the edge is the page's.
    expect(
      tester.widget<UiKitView>(find.byType(UiKitView)).gestureRecognizers,
      isEmpty,
    );
    expect(find.textContaining('open it on a Mac'), findsNothing);
    await native.send('ready', {'page_count': 9});
    await native.send('pageChanged', {'page_1based': 2, 'page_count': 9});
    await pumpFrames(tester);
    expect(find.text('Page 2 of 9'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Search in More opens the find bar at the foot, with the '
      'keyboard; arrows go from match to match, Done closes it', (
    tester,
  ) async {
    final native = _Native();
    await _phone(tester);
    await native.send('ready', {'page_count': 9});
    await pumpFrames(tester);
    expect(find.byType(PhoneFindBar), findsNothing);
    await _openSearch(tester);
    expect(find.byType(PhoneFindBar), findsOneWidget);
    // At the page's foot, the keyboard's way.
    await shots?.call(tester, 'find-open');
    final bar = tester.getRect(find.byType(PhoneFindBar));
    expect(bar.bottom, greaterThan(852 * 0.8));
    expect(tester.testTextInput.isVisible, isTrue);
    // The keyboard rises: the bar with it, above it.
    tester.view.viewInsets = FakeViewPadding(
      bottom: 336 * tester.view.devicePixelRatio,
    );
    addTearDown(tester.view.resetViewInsets);
    await pumpFrames(tester);
    expect(
      tester.getRect(find.byKey(const ValueKey('phone-find-bar'))).bottom,
      lessThanOrEqualTo(852 - 336),
    );

    await tester.enterText(
      find.descendant(
        of: find.byType(PhoneFindBar),
        matching: find.byType(EditableText),
      ),
      'lapwing',
    );
    await tester.pump(const Duration(milliseconds: 300));
    await pumpFrames(tester);
    expect(find.text('1 of 3'), findsOneWidget);
    expect(native.pagesGone().last['page_1based'], 2);
    await shots?.call(tester, 'find-default');

    await tester.tap(find.bySemanticsLabel('Next match'));
    await pumpFrames(tester);
    expect(find.text('2 of 3'), findsOneWidget);
    expect(native.pagesGone().last['page_1based'], 5);
    await tester.tap(find.bySemanticsLabel('Previous match'));
    await tester.tap(find.bySemanticsLabel('Previous match'));
    await pumpFrames(tester);
    expect(find.text('3 of 3'), findsOneWidget);
    expect(native.pagesGone().last['page_1based'], 7);
    // Search on the keyboard: the next, and the keyboard stays.
    await tester.testTextInput.receiveAction(TextInputAction.search);
    await pumpFrames(tester);
    expect(find.text('1 of 3'), findsOneWidget);

    await tester.tap(find.text('Done'));
    await pumpFrames(tester);
    expect(find.byType(PhoneFindBar), findsNothing);
    // The highlights go with it.
    final last = native.calls.lastWhere((c) => c.method == 'search');
    expect((last.arguments as Map)['query'], '');
    expect(tester.takeException(), isNull);
  });

  testWidgets('at the largest text on a small iPhone, the field has a line '
      'of its own and every button is a finger\'s size', (tester) async {
    final native = _Native();
    await _phone(tester, size: const Size(320, 568), textScale: 3.1);
    await native.send('ready', {'page_count': 9});
    await pumpFrames(tester);
    await _openSearch(tester);
    final field = tester.getRect(
      find.descendant(
        of: find.byType(PhoneFindBar),
        matching: find.byType(EditableText),
      ),
    );
    await shots?.call(tester, 'find-largest');
    final next = tester.getRect(find.bySemanticsLabel('Next match'));
    expect(next.top, greaterThanOrEqualTo(field.bottom));
    for (final label in ['Previous match', 'Next match']) {
      final size = tester.getSize(find.bySemanticsLabel(label));
      expect(size.width, greaterThanOrEqualTo(44));
      expect(size.height, greaterThanOrEqualTo(44));
    }
    expect(tester.getSize(find.text('Done')).height, greaterThan(0));
    expect(tester.takeException(), isNull);
  });

  testWidgets('a link out of the PDF asks first', (tester) async {
    final native = _Native();
    final opened = <Uri>[];
    final open = openPdfLink;
    openPdfLink = (url) async {
      opened.add(url);
      return true;
    };
    addTearDown(() => openPdfLink = open);
    await _phone(tester);
    await native.send('linkTapped', {'url': 'https://example.com/birds'});
    await pumpFrames(tester);
    expect(opened, isEmpty);
    expect(find.text('Open example.com?'), findsOneWidget);
    await tester.tap(find.text('Open'));
    await pumpFrames(tester);
    expect(opened, [Uri.parse('https://example.com/birds')]);
  });

  testWidgets('below iOS 16 the page says to update', (tester) async {
    final native = _Native();
    await _phone(tester);
    await native.send('error', {'code': 'os_too_old'});
    await pumpFrames(tester);
    expect(
      find.text('To see PDFs here, update this iPhone to iOS 16 or later.'),
      findsOneWidget,
    );
  });
}
