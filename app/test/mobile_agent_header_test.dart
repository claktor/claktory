// An agent's header on a phone (mobile/agent_header.dart, agent-view.md
// §5): the name over "role · model" on one line, the role cut first, the
// model at its end; "No model available" when it can't run.
import 'package:factory_app/mobile/agent_header.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Future<void> _line(
  WidgetTester tester,
  double width, {
  String role = 'Coordinator',
  String? model = 'Opus 4.8',
}) async {
  // A fresh tree each time: the harness's overlay keeps its first page.
  await tester.pumpWidget(const SizedBox());
  await Harness(Workspace()).show(
    tester,
    size: const Size(393, 852),
    home: Center(
      child: SizedBox(
        width: width,
        child: RoleAndModel(role: role, model: model),
      ),
    ),
  );
  await tester.pump();
}

String _shown(WidgetTester tester) =>
    [for (final text in tester.widgetList<Text>(find.byType(Text))) text.data]
        .join();

void main() {
  testWidgets('room for both: the role, then the model', (tester) async {
    await _line(tester, 300);
    expect(_shown(tester), 'Coordinator · Opus 4.8');
    expect(tester.takeException(), isNull);
  });

  testWidgets('short of room, the role is cut first; then only the model, '
      'cut at its end', (tester) async {
    // Widths as this run's font draws them.
    await _line(tester, 1000);
    final role = tester.getSize(find.text('Coordinator')).width;
    final model = tester.getSize(find.text('Opus 4.8')).width;
    final separator = tester.getSize(find.text(' · ')).width;

    await _line(tester, model + separator + role * 0.6);
    expect(
      _shown(tester),
      'Coordinator · Opus 4.8',
      reason: 'the role ellipsized, the model whole',
    );
    expect(tester.getSize(find.text('Coordinator')).width, lessThan(role));
    expect(tester.getSize(find.text('Opus 4.8')).width, model);

    await _line(tester, model * 0.8);
    expect(_shown(tester), 'Opus 4.8');
    expect(tester.takeException(), isNull);
  });

  testWidgets('no role: the model alone; no model: say so, shorter when '
      "it must", (tester) async {
    await _line(tester, 300, role: '');
    expect(_shown(tester), 'Opus 4.8');
    await _line(tester, 300, role: '', model: null);
    expect(_shown(tester), 'No model available');
    await _line(tester, 70, model: null);
    expect(_shown(tester), 'No model');
    expect(tester.takeException(), isNull);
  });
}
