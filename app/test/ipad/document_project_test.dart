// A project's PDF on an iPad, opened from its Files with no agent
// (ipad/document_host.dart's ipadProjectDocument): the document host, only
// to read, so an iPad never says PDFs open elsewhere. Drawings kept on it
// show; nothing says whether they're kept. An iPhone keeps its
// own PDF page (viewers/pdf.dart).
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/mobile/device.dart';
import 'package:factory_app/mobile/drawer.dart'
    show PhoneDrawerHost, PhoneDrawerHostState;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:factory_app/viewers/pdf.dart' show PdfViewer;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart' show Harness;
import '../panes_test.dart' show snapshot;
import 'as_ipad.dart';
import 'document_native.dart';

/// The app in a window at the phone's routes, a project's Files open on
/// [documentPath]'s folder, and its PDF tapped.
Future<Harness> _openFromFiles(
  WidgetTester tester, {
  required Size size,
}) async {
  final harness = Harness(snapshot());
  harness.backend
    ..files[documentPath] = documentPdf()
    ..explorer.apply(
      Explorer(
        projectId: 'p1',
        entries: [FileEntry(path: 'paper.pdf', name: 'paper.pdf')],
      ),
    );
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(focus: focus, child: const Shell()),
  );
  await settle(tester);
  tester
      .state<PhoneDrawerHostState>(find.byType(PhoneDrawerHost))
      .showFiles('p1');
  await settle(tester);
  await tester.tap(find.text('paper.pdf').last);
  await settle(tester);
  return harness;
}

void main() {
  testWidgets('on an iPad it opens in the host, only to read: nothing '
      'draws, no tools, no review, and Search is in More', (tester) async {
    asIPad();
    final native = DocumentNative();
    // A drawing kept on it, from a review.
    native.ink.otherDevice(2, source: documentPdf().sha256);
    await _openFromFiles(tester, size: const Size(375, 1024));
    expect(find.textContaining('open it on a Mac'), findsNothing);
    expect(native.created.single['drawing_policy'], 'none');
    expect(find.byKey(const ValueKey('ink-finger')), findsNothing);
    expect(find.byKey(const ValueKey('document-review')), findsNothing);
    expect(find.byKey(const ValueKey('document-status')), findsNothing);
    await native.ready();
    await settle(tester);
    // Shown, with nothing said about keeping it.
    final loaded = native.calls.where((c) => c.method == 'loadDrawing');
    expect(loaded, hasLength(1));
    expect(find.byKey(const ValueKey('document-status')), findsNothing);
    await tester.tap(find.bySemanticsLabel('More').last);
    await settle(tester);
    expect(find.text('Search'), findsOneWidget);
    // The Files page under it fits its bar at 375 pt.
    expect(tester.takeException(), isNull);
  });

  testWidgets('on an iPhone it keeps its own PDF page', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    debugDeviceCapabilities = const DeviceCapabilities(idiom: Idiom.phone);
    addTearDown(() {
      debugDefaultTargetPlatformOverride = null;
      debugDeviceCapabilities = null;
    });
    DocumentNative();
    await _openFromFiles(tester, size: const Size(393, 852));
    expect(find.byType(PdfViewer), findsOneWidget);
    expect(find.byKey(const ValueKey('find-panel')), findsNothing);
    // An iPhone's Files page at 393 pt keeps Show hidden files' label.
    expect(find.text('Show hidden files', skipOffstage: false), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
