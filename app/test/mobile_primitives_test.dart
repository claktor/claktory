// The phone's building blocks (research mobile.md, phase 0; #198): its
// sizes and type, the keyboard each kind of field brings, native text
// selection with our own items, the header, haptics and the drawer's
// gesture hold. Each is checked on an iPhone and, where it differs, on a
// Mac, which must stay as it was.
import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoAdaptiveTextSelectionToolbar,
        DefaultCupertinoLocalizations,
        cupertinoTextSelectionHandleControls;
import 'package:factory_app/controls.dart';
import 'package:factory_app/mobile/haptics.dart';
import 'package:factory_app/mobile/header.dart';
import 'package:factory_app/mobile/input.dart';
import 'package:factory_app/mobile/metrics.dart';
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/mobile/presentation.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' show ViewLeaf;
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart' show pumpFrames;
import 'phone_sizes_test.dart' show loadGeist;

/// An app around [child], with the words iOS's text toolbar needs, as
/// main.dart gives it.
Widget _app(Widget child) => WidgetsApp(
  color: const Color(0xFF000000),
  localizationsDelegates: const [
    DefaultCupertinoLocalizations.delegate,
    DefaultWidgetsLocalizations.delegate,
  ],
  builder: (context, _) =>
      Overlay(initialEntries: [OverlayEntry(builder: (context) => child)]),
);

/// An iPhone 16 (393×852, its status bar and home indicator), or a Mac's
/// window with [phone] false.
Future<void> _show(
  WidgetTester tester,
  Widget child, {
  bool phone = true,
  double textScale = 1,
}) async {
  debugDefaultTargetPlatformOverride = phone
      ? TargetPlatform.iOS
      : TargetPlatform.macOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  tester.view.physicalSize = phone
      ? const Size(393, 852)
      : const Size(1200, 900);
  tester.view.devicePixelRatio = 1;
  if (phone) tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
  tester.platformDispatcher.textScaleFactorTestValue = textScale;
  addTearDown(tester.view.reset);
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  await tester.pumpWidget(_app(child));
}

/// Keeps the [BuildContext] under the app, for calls that need one.
class _Context extends StatelessWidget {
  const _Context(this.found);

  final ValueChanged<BuildContext> found;

  @override
  Widget build(BuildContext context) {
    found(context);
    return const SizedBox.expand();
  }
}

/// A phone's navigation that only counts its gesture holds.
class _Routes implements PhoneRoutes {
  @override
  void pushApp(ViewLeaf leaf) {}

  var held = 0;

  @override
  DrawerHold holdGestures() {
    held++;
    return _Hold(this);
  }

  @override
  bool get drawerOpen => false;
  @override
  void openDrawer() {}
  @override
  void closeDrawer() {}
  @override
  Future<T?> push<T>(PhoneRoute<T> route) async => null;
  @override
  void pop<T>([T? result]) {}
  @override
  bool get canPop => false;
  @override
  void openConversation(String agentId, {int? seq}) {}

  @override
  bool showFile(String agentId, String path) => false;

  @override
  bool showPane(String agentId, String paneId) => false;
}

class _Hold implements DrawerHold {
  _Hold(this.routes);

  final _Routes routes;
  var _released = false;

  @override
  void release() {
    if (_released) return;
    _released = true;
    routes.held--;
  }
}

void main() {
  group('sizes and type', () {
    testWidgets('a phone gets 44pt targets, 52pt rows and 17pt text', (
      tester,
    ) async {
      late BuildContext context;
      await _show(tester, _Context((found) => context = found));
      final metrics = Metrics.of(context);
      expect(metrics.phone, isTrue);
      expect(metrics.target, 44);
      expect(metrics.control, 44);
      expect(metrics.row, 52);
      expect(metrics.gutter, 16);
      final type = TypeScale.of(context);
      expect(type.body.fontSize, 17);
      expect(type.secondary.fontSize, 15);
      expect(type.caption.fontSize, 13);
      expect(type.label.fontSize, 17);
      expect(type.title.fontSize, 22);
      expect(type.code.fontSize, 15);
      expect(type.codeInline.fontSize, 16);
      // Nothing smaller than a caption: the HIG's smallest readable size.
      for (final style in [
        type.body,
        type.secondary,
        type.caption,
        type.label,
        type.emphasis,
        type.title,
        type.code,
        type.codeInline,
      ]) {
        expect(style.fontSize, greaterThanOrEqualTo(13));
      }
    });

    testWidgets('a Mac keeps its own', (tester) async {
      late BuildContext context;
      await _show(tester, _Context((found) => context = found), phone: false);
      expect(Metrics.of(context).phone, isFalse);
      expect(Metrics.of(context).target, 28);
      expect(Metrics.of(context).row, 32);
      expect(TypeScale.of(context).body.fontSize, 13);
      expect(TypeScale.of(context).code.fontSize, 12);
    });

    testWidgets('a phone on its side is still a phone', (tester) async {
      late BuildContext context;
      await _show(tester, _Context((found) => context = found));
      tester.view.physicalSize = const Size(852, 393);
      await tester.pump();
      expect(Metrics.of(context).phone, isTrue);
    });

    testWidgets('text grows with Dynamic Type, unclamped', (tester) async {
      for (final scale in [1.0, 2.0, 3.1]) {
        await _show(
          tester,
          Builder(
            builder: (context) =>
                Center(child: Text('Mike', style: TypeScale.of(context).body)),
          ),
          textScale: scale,
        );
        // Geist isn't loaded here, so a line is the font size tall.
        expect(
          tester.getSize(find.text('Mike')).height,
          moreOrLessEquals(17 * 1.4 * scale, epsilon: 1),
          reason: 'at ${scale}x',
        );
      }
    });
  });

  group('controls on a phone', () {
    testWidgets("a button's words grow only to 2×, so a long word stays "
        'whole on a 320pt screen', (tester) async {
      await tester.runAsync(loadGeist);
      await _show(
        tester,
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: Center(
            child: SizedBox(
              width: 288,
              child: Button(
                label: 'Send a test notification',
                onPressed: () {},
              ),
            ),
          ),
        ),
        textScale: 3.1,
      );
      final label = find.text('Send a test notification');
      expect(MediaQuery.textScalerOf(tester.element(label)).scale(10), 20);
      expect(tester.takeException(), isNull);
      // "notification" on one line of its own, not broken.
      final paragraph = tester.renderObject<RenderParagraph>(label);
      const text = 'Send a test notification';
      final boxes = paragraph.getBoxesForSelection(
        TextSelection(
          baseOffset: text.indexOf('notification'),
          extentOffset: text.length,
        ),
      );
      expect(boxes, hasLength(1));
    });

    testWidgets('every button is a finger wide, every one labelled', (
      tester,
    ) async {
      final semantics = tester.ensureSemantics();
      await _show(
        tester,
        Center(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Button(label: 'Save', onPressed: () {}),
              GlyphButton(
                label: 'Attach files',
                active: false,
                onTap: () {},
                glyph: (color) => const SizedBox.square(dimension: 16),
              ),
              Toggle(value: false, label: 'Show hidden', onChanged: (_) {}),
              Select<String>(
                value: 'a',
                title: 'Model',
                options: const [
                  SelectOption('a', 'Opus'),
                  SelectOption('b', 'Sonnet'),
                ],
                onChanged: (_) {},
              ),
            ],
          ),
        ),
      );
      await expectLater(tester, meetsGuideline(iOSTapTargetGuideline));
      await expectLater(tester, meetsGuideline(labeledTapTargetGuideline));
      semantics.dispose();
    });

    testWidgets('a Mac keeps its 28pt glyph buttons', (tester) async {
      await _show(
        tester,
        Center(
          child: GlyphButton(
            label: 'Attach files',
            active: false,
            onTap: () {},
            glyph: (color) => const SizedBox.square(dimension: 16),
          ),
        ),
        phone: false,
      );
      expect(tester.getSize(find.byType(GlyphButton)), const Size(28, 28));
    });
  });

  group('the header', () {
    testWidgets('44pt buttons, a two-line title, its tint and its line', (
      tester,
    ) async {
      final semantics = tester.ensureSemantics();
      var tapped = '';
      await _show(
        tester,
        Align(
          alignment: Alignment.topCenter,
          child: PhoneHeader(
            leading: HeaderButton.menu(onTap: () => tapped = 'menu'),
            title: const Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [Text('Mike'), Text('Coordinator')],
            ),
            titleLabel: 'Mike, Coordinator',
            onTitleTap: () => tapped = 'title',
            trailing: [HeaderButton.more(label: 'Extensions', onTap: () {})],
            background: const Color(0xFF2F6FEB),
            bottom: const SizedBox(key: Key('line'), height: 2),
          ),
        ),
      );
      final header = tester.getRect(find.byType(PhoneHeader));
      expect(header.height, greaterThanOrEqualTo(44));
      for (final button in find.byType(HeaderButton).evaluate()) {
        final size = tester.getSize(find.byWidget(button.widget));
        expect(size.width, greaterThanOrEqualTo(44));
        expect(size.height, greaterThanOrEqualTo(44));
      }
      expect(find.text('Coordinator'), findsOneWidget);
      expect(
        tester.getRect(find.byKey(const Key('line'))).bottom,
        moreOrLessEquals(header.bottom, epsilon: 1.5),
        reason: 'the colored line runs along its bottom',
      );
      await tester.tap(find.text('Mike'));
      expect(tapped, 'title');
      await tester.tap(find.bySemanticsLabel('Menu'));
      expect(tapped, 'menu');
      await expectLater(tester, meetsGuideline(iOSTapTargetGuideline));
      await expectLater(tester, meetsGuideline(labeledTapTargetGuideline));
      semantics.dispose();
    });

    testWidgets('it grows with Dynamic Type rather than clipping', (
      tester,
    ) async {
      await _show(
        tester,
        Align(
          alignment: Alignment.topCenter,
          child: PhoneHeader(
            leading: HeaderButton.back(onTap: () {}),
            title: Builder(
              builder: (context) => Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text('Mike', style: TypeScale.of(context).label),
                  Text('Coordinator', style: TypeScale.of(context).caption),
                ],
              ),
            ),
          ),
        ),
        textScale: 3.1,
      );
      expect(tester.takeException(), isNull, reason: 'no overflow');
      final title = tester.getRect(find.text('Mike'));
      final role = tester.getRect(find.text('Coordinator'));
      final header = tester.getRect(find.byType(PhoneHeader));
      expect(header.height, greaterThan(44));
      expect(header.top, lessThanOrEqualTo(title.top));
      expect(header.bottom, greaterThanOrEqualTo(role.bottom));
    });

    testWidgets('its title grows only so far: a long word stays whole on a '
        'small iPhone at the largest text', (tester) async {
      await _show(
        tester,
        Align(
          alignment: Alignment.topCenter,
          child: SizedBox(
            width: 320,
            child: PhoneHeader(
              leading: HeaderButton.back(onTap: () {}),
              title: Builder(
                builder: (context) =>
                    Text('Notifications', style: TypeScale.of(context).label),
              ),
            ),
          ),
        ),
        textScale: 3.1,
      );
      expect(tester.takeException(), isNull);
      final title = find.text('Notifications');
      // (The test font's square glyphs are wider than Geist's: with Geist
      // the word is one line at 320 pt; here only the cap is checked.)
      expect(
        MediaQuery.textScalerOf(tester.element(title)).scale(1),
        phoneTitleTextScale,
      );
    });
  });

  group('keyboards', () {
    Future<EditableText> field(WidgetTester tester, InputKind kind) async {
      await _show(
        tester,
        Center(
          child: TextField(controller: TextEditingController(), kind: kind),
        ),
      );
      return tester.widget<EditableText>(find.byType(EditableText));
    }

    testWidgets('an email address', (tester) async {
      final editable = await field(tester, InputKind.email);
      expect(editable.keyboardType, TextInputType.emailAddress);
      expect(editable.autofillHints, contains(AutofillHints.email));
      expect(editable.autocorrect, isFalse);
      expect(editable.textCapitalization, TextCapitalization.none);
    });

    testWidgets('a code from an email or a text', (tester) async {
      final editable = await field(tester, InputKind.oneTimeCode);
      expect(editable.keyboardType, TextInputType.number);
      expect(editable.autofillHints, contains(AutofillHints.oneTimeCode));
      await tester.enterText(find.byType(EditableText), '12a3 45-6');
      expect(editable.controller.text, '123456', reason: 'digits only');
    });

    testWidgets('a message: Return starts a new line', (tester) async {
      final editable = await field(tester, InputKind.message);
      expect(editable.keyboardType, TextInputType.multiline);
      expect(editable.textInputAction, TextInputAction.newline);
      expect(editable.textCapitalization, TextCapitalization.sentences);
    });

    testWidgets('a secret is hidden and never suggested', (tester) async {
      final editable = await field(tester, InputKind.secret);
      expect(editable.obscureText, isTrue);
      expect(editable.autocorrect, isFalse);
      expect(editable.enableSuggestions, isFalse);
    });

    testWidgets('a field is 17pt and a finger tall', (tester) async {
      await field(tester, InputKind.text);
      final editable = tester.widget<EditableText>(find.byType(EditableText));
      expect(editable.style.fontSize, 17);
      expect(
        tester.getSize(find.byType(TextField)).height,
        greaterThanOrEqualTo(44),
      );
    });
  });

  group('native selection', () {
    testWidgets('a field offers Paste and our own items, with handles', (
      tester,
    ) async {
      final controller = TextEditingController(text: 'hello world');
      var commented = false;
      await _show(
        tester,
        Center(
          child: TextField(
            controller: controller,
            editItems: (state) => [
              ContextMenuButtonItem(
                label: 'Comment',
                onPressed: () => commented = true,
              ),
            ],
          ),
        ),
      );
      final editable = tester.widget<EditableText>(find.byType(EditableText));
      expect(editable.selectionControls, isNotNull, reason: 'iOS handles');
      expect(editable.contextMenuBuilder, isNotNull, reason: 'a toolbar');
      expect(
        editable.magnifierConfiguration,
        isNot(TextMagnifierConfiguration.disabled),
      );
      // Something to paste, so iOS offers Paste.
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async => switch (call.method) {
          'Clipboard.hasStrings' => {'value': true},
          'Clipboard.getData' => {'text': 'pasted'},
          _ => null,
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        ),
      );
      await tester.longPress(find.byType(EditableText));
      await pumpFrames(tester);
      // The toolbar pages what doesn't fit, so read what it holds.
      final toolbar = tester.widget<CupertinoAdaptiveTextSelectionToolbar>(
        find.byType(CupertinoAdaptiveTextSelectionToolbar),
      );
      final items = {
        for (final item in toolbar.buttonItems!)
          item.label ?? item.type.name: item,
      };
      expect(items.keys, containsAll(['paste', 'Comment']));
      expect(find.text('Paste'), findsOneWidget);
      items['Comment']!.onPressed!();
      expect(commented, isTrue);
    });

    testWidgets('read-only text: Copy and our own items', (tester) async {
      await _show(
        tester,
        // Built once the platform is iOS.
        Builder(
          builder: (context) => Center(
            child: SelectableRegion(
              selectionControls: cupertinoTextSelectionHandleControls,
              contextMenuBuilder: nativeSelectableMenu(
                extra: (state) => [
                  ContextMenuButtonItem(label: 'Suggest', onPressed: () {}),
                ],
              ),
              child: const Text('A passage an agent wrote.'),
            ),
          ),
        ),
      );
      await tester.longPress(find.text('A passage an agent wrote.'));
      await pumpFrames(tester);
      final toolbar = tester.widget<CupertinoAdaptiveTextSelectionToolbar>(
        find.byType(CupertinoAdaptiveTextSelectionToolbar),
      );
      expect([
        for (final item in toolbar.buttonItems!) item.label ?? item.type.name,
      ], containsAll(['copy', 'Suggest']));
    });

    testWidgets('a Mac keeps its own selection', (tester) async {
      await _show(
        tester,
        Center(child: TextField(controller: TextEditingController())),
        phone: false,
      );
      late BuildContext context;
      context = tester.element(find.byType(TextField));
      expect(nativeTextSelection(context).controls, isNull);
      expect(nativeSelectableMenu(), isNull);
    });
  });

  group('haptics', () {
    List<String> listen(WidgetTester tester) {
      final played = <String>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'HapticFeedback.vibrate') {
            played.add(call.arguments as String);
          }
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        ),
      );
      return played;
    }

    testWidgets('an iPhone feels them', (tester) async {
      final played = listen(tester);
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      await Haptics.selection();
      await Haptics.light();
      await Haptics.impact();
      expect(played, [
        'HapticFeedbackType.selectionClick',
        'HapticFeedbackType.lightImpact',
        'HapticFeedbackType.mediumImpact',
      ]);
    });

    testWidgets('a Mac plays none', (tester) async {
      final played = listen(tester);
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      await Haptics.selection();
      await Haptics.success();
      expect(played, isEmpty);
    });
  });

  group('the drawer gesture', () {
    testWidgets('a finger on its own sideways drag holds it off', (
      tester,
    ) async {
      final routes = _Routes();
      await _show(
        tester,
        PhoneRoutesScope(
          routes: routes,
          version: 0,
          child: const Center(
            child: NoDrawerGesture(
              child: SizedBox(
                key: Key('code'),
                width: 300,
                height: 100,
                child: ColoredBox(color: Color(0xFF111111)),
              ),
            ),
          ),
        ),
      );
      final finger = await tester.startGesture(
        tester.getCenter(find.byKey(const Key('code'))),
      );
      expect(routes.held, 1);
      await finger.moveBy(const Offset(-80, 0));
      expect(routes.held, 1);
      await finger.up();
      expect(routes.held, 0, reason: 'given back when the finger lifts');
      final cancelled = await tester.startGesture(
        tester.getCenter(find.byKey(const Key('code'))),
      );
      expect(routes.held, 1);
      await cancelled.cancel();
      expect(routes.held, 0);
    });

    testWidgets('off a phone there is no drawer to hold', (tester) async {
      await _show(
        tester,
        const Center(
          child: NoDrawerGesture(
            child: SizedBox(key: Key('code'), width: 300, height: 100),
          ),
        ),
        phone: false,
      );
      await tester.tap(find.byKey(const Key('code')));
      expect(tester.takeException(), isNull);
    });
  });

  group('sheets', () {
    testWidgets('what is behind a sheet is out of VoiceOver\'s reach', (
      tester,
    ) async {
      final semantics = tester.ensureSemantics();
      late BuildContext context;
      await _show(
        tester,
        Stack(
          children: [
            Center(
              child: Button(label: 'Behind', onPressed: () {}),
            ),
            _Context((found) => context = found),
          ],
        ),
      );
      expect(find.bySemanticsLabel('Behind'), findsOneWidget);
      showPanel(
        context,
        title: 'Checklist',
        builder: (context, close) => const Text('Body'),
      );
      await pumpFrames(tester);
      expect(find.text('Body'), findsOneWidget);
      expect(find.bySemanticsLabel('Behind'), findsNothing);
      expect(find.bySemanticsLabel('Close'), findsWidgets);
      await expectLater(tester, meetsGuideline(iOSTapTargetGuideline));
      semantics.dispose();
    });

    testWidgets('with Reduce Motion it fades rather than slides', (
      tester,
    ) async {
      late BuildContext context;
      await _show(tester, _Context((found) => context = found));
      tester.platformDispatcher.accessibilityFeaturesTestValue =
          FakeAccessibilityFeatures.allOn;
      addTearDown(
        tester.platformDispatcher.clearAccessibilityFeaturesTestValue,
      );
      await tester.pump();
      showPanel(
        context,
        title: 'Checklist',
        builder: (context, close) => const Text('Body'),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 16));
      final resting = tester.getRect(find.byType(PhoneSheet));
      await pumpFrames(tester);
      expect(
        tester.getRect(find.byType(PhoneSheet)),
        resting,
        reason: 'it never moved; it only faded in',
      );
    });

    testWidgets('a sheet of actions is 44pt rows and a Cancel', (tester) async {
      final semantics = tester.ensureSemantics();
      late BuildContext context;
      await _show(tester, _Context((found) => context = found));
      final chosen = showActions<String>(
        context,
        title: 'Claktory',
        actions: const [
          SheetAction('Files', 'files'),
          SheetAction('Project settings', 'settings'),
        ],
      );
      await pumpFrames(tester);
      await expectLater(tester, meetsGuideline(iOSTapTargetGuideline));
      await expectLater(tester, meetsGuideline(labeledTapTargetGuideline));
      await tester.tap(find.text('Project settings'));
      await pumpFrames(tester);
      expect(await chosen, 'settings');
      semantics.dispose();
    });
  });
}
