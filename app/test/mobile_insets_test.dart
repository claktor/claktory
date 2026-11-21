// One thing owns the keyboard and the safe areas on a phone
// (mobile/navigation.dart, "Insets"): the page or the sheet that shows
// something keeps it clear of them, and what's inside sees neither, so
// nothing adds them twice.
import 'dart:async';

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/mobile/presentation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_test.dart' show phone, workspace;

void main() {
  testWidgets("a page's and a sheet's content see neither the keyboard nor "
      'the safe areas, which they own', (tester) async {
    final harness = Harness(workspace());
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    tester.view
      ..padding = const FakeViewPadding(top: 47, bottom: 34)
      ..viewInsets = const FakeViewPadding(bottom: 300);
    addTearDown(tester.view.resetPadding);
    addTearDown(tester.view.resetViewInsets);
    await pumpFrames(tester);

    MediaQueryData? page;
    unawaited(
      PhoneRoutes.of(tester.element(find.byType(AgentView)))!.push(
        PhoneRoute(
          id: 'insets',
          builder: (context) {
            page = MediaQuery.of(context);
            return const Text('A page');
          },
        ),
      ),
    );
    await pumpFrames(tester);
    expect(page!.viewInsets.bottom, 0);
    expect(page!.padding, EdgeInsets.zero);

    MediaQueryData? sheet;
    unawaited(
      presentSheet<void>(
        tester.element(find.text('A page')),
        builder: (context, close) {
          sheet = MediaQuery.of(context);
          return const Text('A sheet');
        },
      ),
    );
    await pumpFrames(tester);
    expect(sheet!.viewInsets.bottom, 0);
    expect(sheet!.padding, EdgeInsets.zero);
  });
}
