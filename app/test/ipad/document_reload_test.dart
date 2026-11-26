// A PDF that changes on the machine while it's open on an iPad
// (ipad/document_host.dart's reload): the new version is fetched and shown
// in place, at the same page; ink on pages that stayed the same stays, and
// ink on pages that changed is set aside, drawn out first as it was, so a
// review's answer still carries it.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart' show Harness;
import 'as_ipad.dart';
import 'document_native.dart';

List<Map<String, Object?>> _sent(DocumentNative native, String method) => [
  for (final call in native.calls)
    if (call.method == method) (call.arguments as Map).cast<String, Object?>(),
];

/// The file becomes [text] on the machine, and the backend says so.
Future<FileContent> _change(
  WidgetTester tester,
  Harness harness, [
  String text = 'a paper on curlews, revised',
]) async {
  final file = documentPdf(text);
  harness.backend
    ..files[documentPath] = file
    ..fileChanged.value = FileChanged(path: documentPath, sha256: file.sha256);
  await settle(tester);
  return file;
}

void main() {
  testWidgets('a new version on the machine is shown in place, and the pane '
      'says so until the person draws', (tester) async {
    asIPad();
    final native = DocumentNative();
    native.reloadWith = (arguments) => {
      'reloaded': true,
      'source_sha256': (arguments['source'] as Map)['sha256'],
      'page_count': 6,
      'page_1based': 4,
      'kept_layers': <Object?>[],
      'outdated_layers': <Object?>[],
    };
    final harness = await showDocument(tester, asking: false);
    await native.ready();
    await settle(tester);

    final file = await _change(tester, harness);
    final reload = _sent(native, 'reloadDocument').single;
    expect((reload['source'] as Map)['sha256'], file.sha256);
    expect((reload['document'] as Map)['sha256'], file.sha256);
    expect(
      find.text('This PDF changed. Showing the new version.'),
      findsOneWidget,
    );
    expect(find.textContaining('Page 4 of 6'), findsOneWidget);

    await native.drew(4, 1);
    await settle(tester);
    expect(find.textContaining('This PDF changed'), findsNothing);
  });

  testWidgets('the same version, another file or a file gone: nothing '
      'reloads', (tester) async {
    asIPad();
    final native = DocumentNative();
    final harness = await showDocument(tester, asking: false);
    await native.ready();
    await settle(tester);
    final same = harness.backend.files[documentPath]!;
    harness.backend.fileChanged.value = FileChanged(
      path: documentPath,
      sha256: same.sha256,
    );
    await settle(tester);
    harness.backend.fileChanged.value = FileChanged(
      path: '/w/other.pdf',
      sha256: 'other',
    );
    await settle(tester);
    harness.backend.fileChanged.value = FileChanged(path: documentPath);
    await settle(tester);
    expect(_sent(native, 'reloadDocument'), isEmpty);
    expect(find.textContaining('This PDF changed'), findsNothing);
  });

  testWidgets('ink on a changed page is set aside, drawn out as it was, and '
      'still goes with the answer beside the new version\'s', (tester) async {
    asIPad();
    final native = DocumentNative();
    native.reloadWith = (arguments) => {
      'reloaded': true,
      'source_sha256': (arguments['source'] as Map)['sha256'],
      'page_count': 5,
      'page_1based': 3,
      'pages_changed': [5],
      'kept_layers': <Object?>[],
      'outdated_layers': [
        {'page_1based': 5, 'ink_layer_id': null, 'revision': null},
      ],
    };
    final harness = await showDocument(tester);
    await native.ready(handwriting: true);
    await native.drew(5, 1);
    await settle(tester);

    await _change(tester, harness);
    // The page with ink drawn out before the new version.
    final methods = [for (final call in native.calls) call.method];
    final reloadAt = methods.indexOf('reloadDocument');
    expect(
      [
        for (final call in native.calls.take(reloadAt))
          if (call.method == 'exportComposite')
            (call.arguments as Map)['page_1based'],
      ],
      [5],
    );
    expect(
      find.text(
        'This PDF changed. Your drawing on page 5 is from the earlier '
        'version.',
      ),
      findsOneWidget,
    );
    // Nothing on the pages now to read as handwriting.
    expect(find.text('Turn handwriting into a comment'), findsNothing);
    // Still said after drawing on the new version.
    await native.drew(3, 3);
    await settle(tester);
    expect(find.textContaining('is from the earlier version'), findsOneWidget);
    expect(find.text('Your drawings on 2 pages go with it.'), findsOneWidget);

    final before = native.calls.length;
    await tester.tap(find.text('Approve'));
    await settle(tester);
    // Page 3 drawn out again, as it is now; page 5 as it was.
    expect(
      [
        for (final call in native.calls.skip(before))
          if (call.method == 'exportComposite')
            (call.arguments as Map)['page_1based'],
      ],
      [3],
    );
    final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
    expect(
      [for (final a in answer.attachments) a.name],
      ['paper-p3.png', 'paper-p5.png'],
    );
  });

  testWidgets('a new version native can\'t show leaves the one shown, '
      'without a word', (tester) async {
    asIPad();
    final native = DocumentNative();
    native.reloadWith = (arguments) =>
        throw PlatformException(code: 'document_too_large');
    final harness = await showDocument(tester, asking: false);
    await native.ready();
    await settle(tester);
    await _change(tester, harness);
    expect(_sent(native, 'reloadDocument'), hasLength(1));
    expect(find.textContaining('This PDF changed'), findsNothing);
    expect(find.textContaining('Page 1 of 5'), findsOneWidget);

    // The next change tries again.
    native.reloadWith = DocumentNative().reloadWith;
    await _change(tester, harness, 'a paper on curlews, revised again');
    expect(_sent(native, 'reloadDocument'), hasLength(2));
    expect(
      find.text('This PDF changed. Showing the new version.'),
      findsOneWidget,
    );
  });

  testWidgets('a search finds again in the new version', (tester) async {
    asIPad();
    final native = DocumentNative();
    final harness = await showDocument(
      tester,
      asking: false,
      size: const Size(2000, 1024),
    );
    await native.ready();
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('document-search')));
    await settle(tester);
    await tester.enterText(find.byType(EditableText).last, 'river');
    await tester.pump(const Duration(milliseconds: 300));
    await settle(tester);
    expect(_sent(native, 'search'), hasLength(1));
    await _change(tester, harness);
    expect(_sent(native, 'search').last['query'], 'river');
    expect(_sent(native, 'search'), hasLength(2));
  });

  testWidgets('ink on a page that stayed the same stays, and is drawn out '
      'as it is when answering', (tester) async {
    asIPad();
    final native = DocumentNative();
    native.reloadWith = (arguments) => {
      'reloaded': true,
      'source_sha256': (arguments['source'] as Map)['sha256'],
      'page_count': 5,
      'page_1based': 3,
      'kept_layers': [
        {'page_1based': 3, 'ink_layer_id': null, 'revision': null},
      ],
      'outdated_layers': [
        {'page_1based': 5, 'ink_layer_id': null, 'revision': null},
      ],
    };
    final harness = await showDocument(tester);
    await native.ready();
    await native.drew(3, 2);
    await native.drew(5, 1);
    await settle(tester);
    await _change(tester, harness);
    expect(
      find.text(
        'This PDF changed. Your drawing on page 5 is from the earlier '
        'version.',
      ),
      findsOneWidget,
    );
    expect(find.text('Your drawings on 2 pages go with it.'), findsOneWidget);
    final before = native.calls.length;
    await tester.tap(find.text('Approve'));
    await settle(tester);
    expect(
      [
        for (final call in native.calls.skip(before))
          if (call.method == 'exportComposite')
            (call.arguments as Map)['page_1based'],
      ],
      [3],
    );
    final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
    expect(
      [for (final a in answer.attachments) a.name],
      ['paper-p3.png', 'paper-p5.png'],
    );
  });
}
