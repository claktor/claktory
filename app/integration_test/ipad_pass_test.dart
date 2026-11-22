// The iPad workbench's phase-1 pass (docs/ipad-workbench.md §4): Claktory
// Dev, built for iPad, against the dev Claktory, with an XCUITest driver
// outside it that rotates the iPad, puts the app in a window and drags it
// to each width, presses keys on the hardware keyboard and takes the
// screenshots. This side pairs, opens the agent with a file beside its
// conversation, then reports what it lays out: whenever the window or the
// workbench changes, and whenever the driver names a step (a line in
// PASS_DIR/mark), a line in PASS_DIR/state.txt says the size, the tier,
// the panes shown and hidden, the one focused, what's open over the
// window, and any layout error, so no screenshot is described without its
// state.
//
//   scripts/ios.sh --dev --device <udid> \
//     --target integration_test/ipad_pass_test.dart \
//     --define PAIRING_CODE='factory://…' --define AGENT=Rivet \
//     --define PASS_FILE=/path/on/the/dev/machine.md \
//     --define PASS_DIR=/tmp/sorbet-pass
//
// (with TARGETED_DEVICE_FAMILY 1,2 as a local edit), then the driver. It
// reports for PASS_MINUTES (default 30) and ends.
import 'dart:io';

import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/explorer.dart' show openFile;
import 'package:factory_app/ipad/ink_surface.dart' show InkSurface;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/message_box.dart' show MessageBox;
import 'package:factory_app/layout.dart' show leaves;
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/pane_chrome.dart' show PaneChrome;
import 'package:factory_app/scope.dart';
import 'package:factory_app/viewers/registry.dart' show ViewerProblem;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _code = String.fromEnvironment('PAIRING_CODE');
const _agent = String.fromEnvironment('AGENT', defaultValue: 'Rivet');
const _file = String.fromEnvironment('PASS_FILE');
const _dir = String.fromEnvironment(
  'PASS_DIR',
  defaultValue: '/tmp/sorbet-pass',
);
const _minutes = int.fromEnvironment('PASS_MINUTES', defaultValue: 30);

/// "document": the PDF pass instead (phase 3): the agent's layout already
/// holds a "pdf" pane; the steps wait for it, have the driver draw on it,
/// and take the shots.
const _only = String.fromEnvironment('PASS_ONLY');

/// With PASS_ONLY=document: "link" taps link.pdf's link (page 1, PDF
/// space x 72–400, y 600–624 on a 612×792 page) with a finger, drawing
/// off; "open" only shows the document (the 2001-page refusal); anything
/// else draws, then answers a review.
const _case = String.fromEnvironment('PASS_CASE');

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('the iPad workbench, as the driver sizes it', (tester) async {
    final state = File('$_dir/state.txt');
    await state.create(recursive: true);
    void note(String text) => state.writeAsStringSync(
      '${DateTime.now().toIso8601String().substring(11, 23)} $text\n',
      mode: FileMode.append,
    );

    Future<void> settle([int ms = 600]) async {
      final end = DateTime.now().add(Duration(milliseconds: ms));
      while (DateTime.now().isBefore(end)) {
        await tester.pump(const Duration(milliseconds: 50));
        await Future<void>.delayed(const Duration(milliseconds: 50));
      }
    }

    Future<void> until(Finder finder, {int seconds = 120}) async {
      final end = DateTime.now().add(Duration(seconds: seconds));
      while (finder.evaluate().isEmpty) {
        if (DateTime.now().isAfter(end)) fail('never found $finder');
        await settle(200);
      }
    }

    await app.main();
    await settle(1500);
    if (find.text('Paste code instead').evaluate().isNotEmpty ||
        find.text('Connect to your Claktory').evaluate().isNotEmpty) {
      expect(_code, isNotEmpty, reason: 'pass --define PAIRING_CODE=…');
      for (var i = 0; i < 25; i++) {
        if (find.byType(EditableText).evaluate().isNotEmpty) break;
        if (find.text('Paste code instead').evaluate().isNotEmpty) {
          await tester.tap(find.text('Paste code instead'));
        }
        await settle(200);
      }
      await tester.enterText(find.byType(EditableText).first, _code);
      await settle(300);
      await tester.tap(find.text('Connect').last);
    }
    final shell = find.byWidgetPredicate(
      (widget) =>
          widget is Workbench || widget.runtimeType.toString() == 'PhoneShell',
    );
    await until(shell);
    if (find.text("What's your name?").evaluate().isNotEmpty) {
      await tester.enterText(find.byType(EditableText).last, 'Matt');
      await tester.tap(find.text('Continue'));
      await settle(1500);
    }

    final scope = AppScope.scope(tester.element(shell.first));
    final backend = scope.backend;
    for (var i = 0; i < 300; i++) {
      if (backend.workspace.workspace.agents.any((a) => a.name == _agent)) {
        break;
      }
      await settle(200);
    }
    final agent = backend.workspace.workspace.agents.firstWhere(
      (each) => each.name == _agent,
    );
    await backend.selectAgent(agent.id);
    await settle(1000);
    if (_file.isNotEmpty) {
      await backend.setLayout(agent.id, null);
      await settle(500);
      openFile(tester.element(shell.first), _file);
      await settle(1500);
    }
    note('ready: ${agent.name} (${agent.id}), file "$_file"');

    // Layout errors are what this pass is for: kept, never fatal.
    final errors = <String>[];
    final reported = FlutterError.onError;
    FlutterError.onError = (details) {
      errors.add(
        '${details.exceptionAsString().split('\n').first} '
        '(${details.context?.toDescription() ?? details.library})',
      );
    };
    addTearDown(() => FlutterError.onError = reported);

    // Every key the app hears, as Flutter hears it.
    bool heard(KeyEvent event) {
      if (event is KeyDownEvent) {
        note(
          '  key down ${event.logicalKey.keyLabel} '
          '(${HardwareKeyboard.instance.logicalKeysPressed.map((k) => k.keyLabel).join('+')})',
        );
      }
      return false;
    }

    HardwareKeyboard.instance.addHandler(heard);
    addTearDown(() => HardwareKeyboard.instance.removeHandler(heard));

    String describe() {
      final view = binding.renderViews.first;
      final size = view.size;
      final ratio = view.flutterView.devicePixelRatio;
      final padding = EdgeInsets.fromViewPadding(
        view.flutterView.padding,
        ratio,
      );
      final workbench = find.byType(Workbench);
      final parts = <String>[
        '${size.width.round()}×${size.height.round()}',
        'safe t${padding.top.round()} b${padding.bottom.round()}',
      ];
      if (workbench.evaluate().isEmpty) {
        parts.add('compact (phone routes)');
        final viewer = find.byWidgetPredicate(
          (widget) => widget is PaneChrome && widget.phone != null,
        );
        if (viewer.evaluate().isNotEmpty) {
          parts.add(
            'viewer ${(tester.widget(viewer.last) as PaneChrome).phone!.title}',
          );
        }
      } else {
        final context = tester.element(workbench.first);
        final focus = WorkbenchScope.of(context);
        final textScale = MediaQuery.textScalerOf(context).scale(17) / 17;
        parts.add('tier ${windowTier(context, size).name}');
        parts.add('text ×${textScale.toStringAsFixed(2)}');
        final projection = focus.projectionOf(agent.id);
        if (projection != null) {
          parts.add(
            'shown ${projection.shown == null ? '-' : [for (final leaf in leaves(projection.shown!)) leaf.id]}',
          );
          parts.add('hidden ${projection.hidden}');
          parts.add('focused ${projection.focused}');
        }
        parts.add('menu ${focus.menu.native ? 'native' : 'flutter'}');
        if (focus.sidebarHidden) parts.add('sidebar hidden');
        if (focus.overlayOpen) parts.add('sidebar over');
      }
      final keyboard = MediaQuery.of(tester.element(shell.first))
          .viewInsets
          .bottom;
      if (keyboard > 0) parts.add('keyboard ${keyboard.round()}');
      if (find.text('Panes').evaluate().isNotEmpty) parts.add('Panes open');
      final primary = FocusManager.instance.primaryFocus;
      parts.add(
        'keys to ${primary?.debugLabel ?? primary?.context?.widget.runtimeType}',
      );
      if (find
          .byWidgetPredicate(
            (widget) => widget.runtimeType.toString() == 'NameStepBarrier',
          )
          .evaluate()
          .isNotEmpty) {
        parts.add('A SETUP STEP COVERS THE WINDOW');
      }
      if (errors.isNotEmpty) {
        parts.add('ERRORS ${errors.toSet().join(' / ')}');
        errors.clear();
      }
      return parts.join(', ');
    }

    // A screenshot of the whole screen, by the runner (`xcrun simctl io
    // <udid> screenshot`), asked for with PASS_DIR/request; the state
    // beside it in state.txt.
    Future<void> shot(String name) async {
      await settle(900);
      note('[$name] ${describe()}');
      final request = File('$_dir/request');
      request.writeAsStringSync(name);
      final end = DateTime.now().add(const Duration(seconds: 20));
      while (request.existsSync()) {
        if (DateTime.now().isAfter(end)) {
          note('  no screenshot for $name');
          request.deleteSync();
          return;
        }
        await settle(200);
      }
    }

    // Keys on the hardware keyboard, pressed by the driver through UIKit
    // (so the system's key commands answer as they would for a person).
    Future<void> press(String spec) async {
      final keys = File('$_dir/keys');
      keys.writeAsStringSync(spec);
      final end = DateTime.now().add(const Duration(seconds: 90));
      while (keys.existsSync()) {
        if (DateTime.now().isAfter(end)) {
          note('  nobody pressed $spec');
          keys.deleteSync();
          return;
        }
        await settle(200);
      }
      await settle(600);
    }

    Future<void> step(String name, Future<void> Function() body) async {
      try {
        await body();
      } catch (error) {
        note('SKIPPED $name: ${'$error'.split('\n').first}');
      }
    }

    void unfocus() => FocusManager.instance.primaryFocus?.unfocus();

    Finder keyed(String key) => find.byKey(ValueKey(key));

    // Somewhere no popover or sidebar reaches: the window's bottom right,
    // above the message box.
    Offset outside() {
      final size = binding.renderViews.first.size;
      return Offset(size.width - 24, size.height - 220);
    }

    // The keyboard up over a draft: the message box stays above it.
    Future<void> draft(String prefix) => step('$prefix draft', () async {
      final box = find
          .descendant(
            of: find.byType(MessageBox),
            matching: find.byType(EditableText),
          )
          .last;
      await tester.showKeyboard(box);
      await tester.enterText(
        box,
        'A draft from the iPad pass, a first line\nand a second one.',
      );
      await shot('$prefix-5-draft');
      await tester.enterText(box, '');
      unfocus();
      await settle();
    });

    // Strokes by the driver, real touches through UIKit to PencilKit:
    // from and to as fractions of the window, in PASS_DIR/drag.
    Future<void> drag(Offset from, Offset to) async {
      final size = binding.renderViews.first.size;
      final file = File('$_dir/drag');
      file.writeAsStringSync(
        [
          from.dx / size.width,
          from.dy / size.height,
          to.dx / size.width,
          to.dy / size.height,
        ].map((v) => v.toStringAsFixed(4)).join(','),
      );
      final end = DateTime.now().add(const Duration(seconds: 60));
      while (file.existsSync()) {
        if (DateTime.now().isAfter(end)) {
          note('  nobody drew');
          file.deleteSync();
          return;
        }
        await settle(200);
      }
      await settle(800);
    }

    // A finger tap by the driver, a real touch through UIKit.
    Future<void> tap(Offset at) async {
      final size = binding.renderViews.first.size;
      final file = File('$_dir/tap');
      file.writeAsStringSync(
        '${(at.dx / size.width).toStringAsFixed(4)},'
        '${(at.dy / size.height).toStringAsFixed(4)}',
      );
      final end = DateTime.now().add(const Duration(seconds: 60));
      while (file.existsSync()) {
        if (DateTime.now().isAfter(end)) {
          note('  nobody tapped');
          file.deleteSync();
          return;
        }
        await settle(200);
      }
      await settle(1500);
    }

    Future<void> document(String prefix) async {
      // The driver's touches are real: let them through to the app (and
      // so to PencilKit), which a test binding doesn't by default.
      binding.shouldPropagateDevicePointerEvents = true;
      final surface = find.byType(InkSurface);
      // The focused tier shows one pane: the document's, by its name.
      final named = find.textContaining('.pdf');
      if (surface.evaluate().isEmpty && named.evaluate().isNotEmpty) {
        await tester.tap(named.first);
        await settle();
      }
      final end = DateTime.now().add(const Duration(seconds: 60));
      while (surface.evaluate().isEmpty &&
          find.byType(ViewerProblem).evaluate().isEmpty &&
          DateTime.now().isBefore(end)) {
        await settle(300);
      }
      await settle(2500);
      await shot('$prefix-1-document');
      if (surface.evaluate().isEmpty) {
        note('  $prefix: no surface');
        return;
      }
      final rect = tester.getRect(surface.first);
      note('  $prefix: surface $rect');
      if (_case == 'open') return;
      if (_case == 'link') {
        // PDFKit fits the page's width, a few points in from each side.
        const inset = 6.0;
        final scale = (rect.width - 2 * inset) / 612;
        final at =
            rect.topLeft +
            Offset(inset + 236 * scale, inset + 180 * scale); // its middle
        await tap(at);
        await shot('$prefix-2-link');
        return;
      }
      // A finger scrolls a PDF until Draw with finger is on (the
      // simulator has no Pencil).
      await tester.tap(keyed('ink-finger'));
      await settle();
      await shot('$prefix-2-finger');
      await drag(
        rect.topLeft + Offset(rect.width * .2, rect.height * .3),
        rect.topLeft + Offset(rect.width * .7, rect.height * .35),
      );
      await drag(
        rect.topLeft + Offset(rect.width * .25, rect.height * .45),
        rect.topLeft + Offset(rect.width * .6, rect.height * .55),
      );
      await shot('$prefix-3-drawn');
      // A review the agent asked of the file (the runner has it ask):
      // the bar at the pane's foot, a comment, then Approve.
      final review = keyed('document-review');
      final waitUntil = DateTime.now().add(const Duration(seconds: 30));
      while (review.evaluate().isEmpty && DateTime.now().isBefore(waitUntil)) {
        await settle(300);
      }
      if (review.evaluate().isEmpty) {
        note('  $prefix: no review asked');
        return;
      }
      await shot('$prefix-4-review');
      await tester.tap(find.text('Add a comment'));
      await settle();
      await tester.enterText(
        find.descendant(of: review, matching: find.byType(EditableText)),
        'The lapwing count looks low; see my marks.',
      );
      await shot('$prefix-5-comment');
      unfocus();
      await tester.tap(
        find.descendant(of: review, matching: find.text('Approve')),
      );
      await settle(4000);
      await shot('$prefix-6-answered');
    }

    Future<void> steps(String prefix) async {
      unfocus();
      await settle();
      if (_only == 'document') return document(prefix);
      await shot('$prefix-1-window');
      if (find.byType(Workbench).evaluate().isEmpty) {
        // Compact: the phone's routes, the drawer and the file.
        await step('$prefix drawer', () async {
          await tester.tap(find.bySemanticsLabel('Menu').first);
          await shot('$prefix-2-drawer');
          await press('escape');
          // Esc pressed through UIKit doesn't reach the app in the
          // simulator: the drawer's dimmed edge closes it.
          final size = binding.renderViews.first.size;
          await tester.tapAt(Offset(size.width - 8, size.height / 2));
          await settle();
        });
        await draft(prefix);
        return;
      }
      await step('$prefix sidebar', () async {
        await tester.tap(keyed('workbench-sidebar'));
        await shot('$prefix-2-sidebar');
        // Over the window: Esc closes it; pinned (full): the button again.
        if (keyed('workbench-scrim').evaluate().isNotEmpty) {
          await press('escape');
          if (keyed('workbench-scrim').evaluate().isNotEmpty) {
            note('  $prefix: Esc left the sidebar open; closing it by a tap');
            await tester.tapAt(outside());
            await settle();
          }
        } else {
          await tester.tap(keyed('workbench-sidebar'));
          await settle();
        }
      });
      await step('$prefix panes', () async {
        if (keyed('workbench-panes').evaluate().isEmpty) {
          note('  $prefix: no Panes button (every pane shown)');
          await press('alternate+command+p');
          await shot('$prefix-3-panes-from-keys');
        } else {
          await tester.tap(keyed('workbench-panes'));
          await shot('$prefix-3-panes');
        }
        await press('escape');
        if (find.byType(PanesList).evaluate().isNotEmpty) {
          note('  $prefix: Esc left Panes open; closing it by a tap');
          await tester.tapAt(outside());
          await settle();
        }
      });
      await step('$prefix next pane', () async {
        await press('control+tab');
        await shot('$prefix-4-next-pane');
        await press('control+shift+tab');
      });
      await draft(prefix);
      await step('$prefix sidebar keys', () async {
        await press('control+command+s');
        await shot('$prefix-6-sidebar-keys');
        await press('control+command+s');
      });
      await step('$prefix expand', () async {
        var expand = find.byWidgetPredicate(
          (w) => w is GlyphButton && w.label.startsWith('Expand '),
        );
        if (expand.evaluate().isEmpty) {
          // The focused tier shows one pane: the document first.
          final document = find.text('field-guide.md');
          if (document.evaluate().isNotEmpty) {
            await tester.tap(document.first);
            await settle();
          }
          expand = find.byWidgetPredicate(
            (w) => w is GlyphButton && w.label.startsWith('Expand '),
          );
        }
        if (expand.evaluate().isEmpty) {
          note('  $prefix: no Expand');
          return;
        }
        await tester.tap(expand.first);
        await settle();
        await shot('$prefix-8-expanded');
        await press('escape');
        final back = find.textContaining('Back to ');
        if (back.evaluate().isNotEmpty) {
          note('  $prefix: Esc left the pane expanded; Back by a tap');
          await tester.tap(back.first);
          await settle();
        }
        await shot('$prefix-9-collapsed');
        final conversation = find.text('Conversation');
        if (find.byType(Workbench).evaluate().isNotEmpty &&
            conversation.evaluate().isNotEmpty) {
          await tester.tap(conversation.first);
          await settle();
        }
      });
      await step('$prefix more', () async {
        final more = find.bySemanticsLabel(RegExp(r'^More'));
        if (more.evaluate().isEmpty) {
          note('  $prefix: no More');
          return;
        }
        await tester.tap(more.first);
        await shot('$prefix-7-more');
        await press('escape');
        await tester.tapAt(outside());
        await settle();
      });
    }

    final mark = File('$_dir/mark');
    var last = '';
    var lastMark = '';
    final end = DateTime.now().add(const Duration(minutes: _minutes));
    while (DateTime.now().isBefore(end)) {
      await settle(300);
      final now = describe();
      final named = mark.existsSync() ? mark.readAsStringSync().trim() : '';
      if (named != lastMark) {
        lastMark = named;
        if (named.startsWith('go:')) {
          final prefix = named.substring(3);
          note('== $prefix: $now');
          await steps(prefix);
          File('$_dir/ack').writeAsStringSync('done:$prefix');
        }
      } else if (now != last) {
        note('changed: $now');
      }
      last = now;
      if (named == 'done') break;
    }
  });
}
