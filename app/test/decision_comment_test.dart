// "Add a comment" under a card's buttons opens its field with the focus,
// even while something else has it (Sorbet's iPad find: after a PDF's
// reload its pane held focus, and the field opened without it).
import 'package:factory_app/decision.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

void main() {
  testWidgets('the field opens focused though something else had focus', (
    tester,
  ) async {
    final comment = TextEditingController();
    addTearDown(comment.dispose);
    final other = FocusNode();
    addTearDown(other.dispose);
    await Harness(Workspace()).show(
      tester,
      home: Column(
        children: [
          Focus(focusNode: other, child: const SizedBox(height: 20)),
          DecisionComment(controller: comment, agent: 'June'),
        ],
      ),
    );
    other.requestFocus();
    await tester.pump();
    expect(other.hasFocus, isTrue);

    await tester.tap(find.text('Add a comment'));
    await pumpFrames(tester);
    final field = tester.widget<EditableText>(find.byType(EditableText));
    expect(field.focusNode.hasFocus, isTrue);
    expect(other.hasFocus, isFalse);
    await tester.enterText(find.byType(EditableText), 'Looks good');
    expect(comment.text, 'Looks good');
  });
}
