import 'package:factory_app/android/behavior_policy.dart';
import 'package:factory_app/mobile/input.dart';
import 'package:material_ui/material_ui.dart' show MaterialLocalizations;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  const policy = AndroidBehaviorPolicy();

  test('Android touch and edge policy is independent of available width', () {
    expect(policy.touchTarget, 48);
    expect(policy.drawerEdgeSwipe, isFalse);
    final page = policy.page<void>(
      key: const ValueKey('viewer'),
      child: const SizedBox(),
    );
    expect(page, isA<Page<void>>());
    expect(page.key, const ValueKey('viewer'));
  });

  test(
    'shared input contract preserves composition-friendly multiline setup',
    () {
      final message = InputKind.message.setup;
      expect(message.keyboardType, TextInputType.multiline);
      expect(message.textInputAction, TextInputAction.newline);
      expect(message.inputFormatters, isEmpty);
      expect(
        InputKind.email.setup.autofillHints,
        contains(AutofillHints.email),
      );
      final secret = InputKind.secret.setup;
      expect(secret.obscureText, isTrue);
      expect(secret.enableSuggestions, isFalse);
      expect(secret.autocorrect, isFalse);
    },
  );

  testWidgets('Android handles, toolbar and extra action work in WidgetsApp', (
    tester,
  ) async {
    final controller = TextEditingController(text: 'A draft to comment on');
    final focus = FocusNode();
    addTearDown(controller.dispose);
    addTearDown(focus.dispose);
    var comments = 0;
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xff202020),
        localizationsDelegates: policy.localizations,
        home: Builder(
          builder: (context) {
            expect(MaterialLocalizations.of(context).copyButtonLabel, 'Copy');
            final selection = policy.selection(
              context,
              extra: (state) => [
                ContextMenuButtonItem(
                  label: 'Comment',
                  onPressed: () {
                    comments++;
                    state.hideToolbar();
                  },
                ),
              ],
            );
            expect(selection.controls, isNotNull);
            expect(selection.magnifier.magnifierBuilder, isNotNull);
            return Center(
              child: SizedBox(
                width: 300,
                child: EditableText(
                  controller: controller,
                  focusNode: focus,
                  style: const TextStyle(fontSize: 17),
                  cursorColor: const Color(0xffffffff),
                  backgroundCursorColor: const Color(0xff777777),
                  selectionControls: selection.controls,
                  contextMenuBuilder: selection.contextMenuBuilder,
                  magnifierConfiguration: selection.magnifier,
                ),
              ),
            );
          },
        ),
        pageRouteBuilder: <T>(settings, builder) => PageRouteBuilder<T>(
          settings: settings,
          pageBuilder: (context, _, _) => builder(context),
        ),
      ),
    );
    focus.requestFocus();
    await tester.pump();
    controller.selection = const TextSelection(baseOffset: 2, extentOffset: 7);
    final field = tester.state<EditableTextState>(find.byType(EditableText));
    field.showToolbar();
    await tester.pumpAndSettle();
    expect(find.text('Copy'), findsOneWidget);
    expect(find.text('Comment'), findsOneWidget);
    await tester.tap(find.text('Comment'));
    await tester.pumpAndSettle();
    expect(comments, 1);
    expect(controller.text, 'A draft to comment on');
    expect(find.text('Comment'), findsNothing);
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));
}
