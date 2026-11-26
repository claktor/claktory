// The sidebar footer's row with many providers (Matt, Bruno 41265): who's
// at work, then each account's usage circle and the + to add a provider,
// wrapping together from the row's left edge, each line's cells centred on
// one another, at the narrowest and widest sidebar and at large text, from
// 1 to 6 accounts. With SHOTS set, each case is drawn to a PNG there:
//   SHOTS=/tmp/footer flutter test test/footer_circles_test.dart
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/team_activity.dart';
import 'package:factory_app/usage.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

/// [count] accounts over the providers Matt has: two Claude subscriptions,
/// ChatGPT, OpenRouter, then more Claude ones.
List<Provider> providers(int count) {
  final claude = <Account>[];
  final providers = <Provider>[];
  var left = count;
  Account account(String id) => Account(id: id, label: '$id@example.com');
  for (var i = 0; i < 2 && left > 0; i++, left--) {
    claude.add(account('claude$i'));
  }
  if (left > 0) {
    providers.add(
      Provider(
        id: 'openai-codex',
        name: 'OpenAI Codex',
        accounts: [account('chatgpt')],
      ),
    );
    left--;
  }
  if (left > 0) {
    providers.add(
      Provider(
        id: 'openrouter',
        name: 'OpenRouter',
        accounts: [account('router')],
      ),
    );
    left--;
  }
  for (var i = 2; left > 0; i++, left--) {
    claude.add(account('claude$i'));
  }
  return [
    Provider(id: 'anthropic', name: 'Anthropic', accounts: claude),
    ...providers,
  ];
}

Future<void> _shot(WidgetTester tester, String name, Rect rect) async {
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final view = tester.binding.renderViews.first;
  final layer = view.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(() => layer.toImage(rect, pixelRatio: 2));
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

void main() {
  setUpAll(loadGeist);

  for (final width in [sidebarMinWidth, sidebarMaxWidth]) {
    for (final scale in [1.0, 1.6]) {
      for (var count = 1; count <= 6; count++) {
        testWidgets('${width.toInt()} wide, text ×$scale, $count accounts', (
          tester,
        ) async {
          tester.platformDispatcher.textScaleFactorTestValue = scale;
          addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
          final harness = Harness(
            Workspace(
              agents: [
                Agent(id: 'rivet', name: 'Rivet', modelAvailable: true),
                Agent(
                  id: 'a1',
                  name: 'Kai',
                  modelAvailable: true,
                  status: AgentStatus.AGENT_STATUS_WORKING,
                ),
              ],
              selectedAgentId: 'rivet',
              settings: Settings(sidebarWidth: width),
              providers: providers(count),
            ),
          );
          await harness.show(tester, size: const Size(1200, 800));
          await tester.pump();

          final badge = tester.getRect(find.byType(TeamActivity).first);
          final cells = [
            for (final element in find.byType(UsageCircle).evaluate())
              tester.getRect(find.byWidget(element.widget)),
            tester.getRect(find.byType(AddProviderButton).first),
          ];
          expect(cells, hasLength(count + 1));
          final row = [badge, ...cells];
          // The badge is as wide as its count, the first of the row.
          expect(badge.width, lessThan(80));
          // The footer's row, inside its padding (Space.s, then Space.xs).
          final left = badge.left - Space.s - Space.xs;
          final edge = left + width;
          final top = badge.top - 12;
          final bottom = cells
              .map((cell) => cell.bottom)
              .reduce((a, b) => a > b ? a : b);
          await _shot(
            tester,
            'w${width.toInt()}-x$scale-n$count',
            Rect.fromLTRB(left, top, edge, bottom + 12),
          );
          for (final cell in row) {
            expect(cell.right, lessThanOrEqualTo(edge), reason: '$cell');
          }
          for (var i = 1; i < row.length; i++) {
            final before = row[i - 1];
            final cell = row[i];
            final sameLine = (cell.center.dy - before.center.dy).abs() < 1;
            if (sameLine) {
              expect(cell.left, greaterThan(before.right - 0.5));
            } else {
              // A new line starts at the row's left edge, under the badge,
              // never indented beside it.
              expect(cell.top, greaterThanOrEqualTo(before.bottom - 0.5));
              expect(
                cell.left,
                moreOrLessEquals(badge.left, epsilon: 0.5),
                reason: 'line ${i + 1} starts where the row does',
              );
            }
          }
        });
      }
    }
  }
}
