import 'package:factory_app/database.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

Workspace databaseWorkspace() {
  final snapshot = workspace(
    agents: [
      builder(working: false)
        ..openPanel = 'database'
        ..extensions.add(AgentExtension(id: 'database', enabled: true)),
    ],
  );
  snapshot.extensions.add(
    Extension(id: 'database', name: 'Database', icon: 'database'),
  );
  snapshot.openPanel = 'database';
  return snapshot;
}

Future<Harness> show(WidgetTester tester, Database database) async {
  final harness = Harness(databaseWorkspace());
  harness.backend.extensionView.apply(
    ExtensionView(
      extensionId: 'database',
      agentId: 'a1',
      title: 'Database',
      database: database,
    ),
  );
  await harness.show(tester);
  await tester.pump(const Duration(milliseconds: 500));
  return harness;
}

Finder inPanel(String text) =>
    find.descendant(of: find.byType(DatabaseView), matching: find.text(text));

void main() {
  testWidgets('tables and views with their rows; a click shows columns', (
    tester,
  ) async {
    await show(
      tester,
      Database(
        path: '/Users/me/.factory/agents/a1/data.db',
        tables: [
          DatabaseTable(
            name: 'invoices',
            rows: Int64(12345),
            columns: [
              DatabaseColumn(name: 'id', type: 'INTEGER'),
              DatabaseColumn(name: 'note'),
            ],
          ),
          DatabaseTable(name: 'unpaid', view: true, rows: Int64.ONE),
        ],
      ),
    );
    expect(inPanel('~/.factory/agents/a1/data.db'), findsOneWidget);
    expect(inPanel('Tables'), findsOneWidget);
    expect(inPanel('Views'), findsOneWidget);
    expect(inPanel('invoices'), findsOneWidget);
    expect(inPanel('12,345 rows'), findsOneWidget);
    expect(inPanel('1 row'), findsOneWidget);
    double top(String text) => tester.getTopLeft(inPanel(text)).dy;
    expect(top('invoices'), lessThan(top('Views')));
    expect(top('Views'), lessThan(top('unpaid')));

    expect(inPanel('INTEGER'), findsNothing);
    await tester.tap(inPanel('invoices'));
    await tester.pump();
    expect(inPanel('id'), findsOneWidget);
    expect(inPanel('INTEGER'), findsOneWidget);
    expect(inPanel('note'), findsOneWidget);
    await tester.tap(inPanel('invoices'));
    await tester.pump();
    expect(inPanel('INTEGER'), findsNothing);
  });

  testWidgets('says when there are no tables yet', (tester) async {
    await show(tester, Database(path: '/Users/me/data.db'));
    expect(
      inPanel('No tables yet. The agent makes them when it has data to keep.'),
      findsOneWidget,
    );
    expect(inPanel('Tables'), findsNothing);
  });

  test('row counts', () {
    expect(rowsLabel(Int64.ZERO), '0 rows');
    expect(rowsLabel(Int64(999)), '999 rows');
    expect(rowsLabel(Int64(1000)), '1,000 rows');
    expect(rowsLabel(Int64(1234567)), '1,234,567 rows');
  });
}
