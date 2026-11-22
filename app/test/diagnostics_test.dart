// Settings → Advanced diagnostics (#214): off unless the person turns it
// on; while on, what each source kept (this Claktory and each device), by
// area when it has several, Delete for one source, Delete all after
// asking; turning it off deletes it all, after asking when anything is
// kept. Only from a backend that records them.

import 'package:factory_app/diagnostics.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/settings_view.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;
import 'phone_test.dart' show phone;

final _now = DateTime.now();

Int64 _ago(Duration age) => Int64(_now.subtract(age).millisecondsSinceEpoch);

DiagnosticsArea _area(String area, int bytes) => DiagnosticsArea(
  area: area,
  events: 10,
  bytes: Int64(bytes),
  oldestMs: _ago(const Duration(days: 3, hours: 2)),
  newestMs: Int64(_now.millisecondsSinceEpoch),
);

DiagnosticsSource _source(
  String id,
  String name, {
  int bytes = 84000,
  Duration age = const Duration(days: 3, hours: 2),
  List<DiagnosticsArea>? areas,
}) => DiagnosticsSource(
  id: id,
  name: name,
  events: 312,
  bytes: Int64(bytes),
  oldestMs: _ago(age),
  newestMs: Int64(_now.millisecondsSinceEpoch),
  receivedMs: Int64(_now.millisecondsSinceEpoch),
  areas: areas ?? [_area('connection', bytes)],
);

Workspace _workspace(Diagnostics? diagnostics) => Workspace(
  agents: [Agent(id: 'june', name: 'June', modelAvailable: true)],
  selectedAgentId: 'june',
  version: '2026.11.22-0900',
  remote: RemoteAccess(
    devices: [PairedDevice(id: 'ipad', name: 'Studio iPad')],
  ),
  diagnostics: diagnostics,
);

Diagnostics _diagnostics({
  bool share = true,
  List<DiagnosticsSource> sources = const [],
}) => Diagnostics(
  share: share,
  sources: sources,
  keepDays: 14,
  sourceLimitBytes: Int64(4000000),
);

Future<Harness> _show(WidgetTester tester, Diagnostics? diagnostics) async {
  final harness = Harness(_workspace(diagnostics));
  harness.page.value = AppPage.settings;
  await harness.show(tester, size: const Size(1200, 2400));
  await pumpFrames(tester);
  final scrollable = find
      .descendant(
        of: find.byType(SettingsView),
        matching: find.byType(Scrollable),
      )
      .first;
  await tester.drag(scrollable, const Offset(0, -4000));
  await pumpFrames(tester);
  return harness;
}

SetDiagnostics _lastSet(Harness harness) => harness.backend.sent
    .lastWhere((message) => message.hasSetDiagnostics())
    .setDiagnostics;

Iterable<ClientMessage> _sets(Harness harness) =>
    harness.backend.sent.where((m) => m.hasSetDiagnostics());

void main() {
  setUpAll(loadGeist);

  test('what each source kept, in a line', () {
    const paired = {'ipad': 'Studio iPad'};
    String line(DiagnosticsSource source) =>
        diagnosticsSourceLine(source, _now, paired: paired);
    expect(
      line(_source('device:mac', 'Studio')),
      'Studio · Connections · 84.0 KB over the last 3 days',
    );
    expect(
      line(_source('device:ipad', '', age: const Duration(minutes: 70))),
      'Studio iPad · Connections · 84.0 KB over the last hour',
      reason: "the paired device's name when it sent none",
    );
    expect(
      line(
        _source(
          'device:local',
          '',
          bytes: 950,
          age: const Duration(minutes: 5),
        ),
      ),
      'This computer · Connections · 950 bytes over the last 5 minutes',
    );
    expect(
      line(_source('backend', '')),
      'This Claktory · Connections · 84.0 KB over the last 3 days',
    );
    expect(
      line(_source('device:gone', '', areas: [])),
      'A device · 84.0 KB over the last 3 days',
    );
    expect(
      line(
        _source(
          'backend',
          '',
          areas: [_area('connection', 60000), _area('updates', 24000)],
        ),
      ),
      'This Claktory · 84.0 KB over the last 3 days',
      reason: 'several areas show under the line',
    );
    expect(diagnosticsAreaName('connection'), 'Connections');
    expect(diagnosticsAreaName(''), 'General');
    expect(diagnosticsAreaName('app_views'), 'App views');
    expect(diagnosticsNote(14), endsWith('Kept on this Claktory for 14 days.'));
    expect(
      diagnosticsNote(14),
      'Claktory and your devices record details of how they work, such as '
      'when a connection drops and why, so your agents can find out what '
      'goes wrong. Never your messages, codes or network addresses. Kept on '
      'this Claktory for 14 days.',
    );
    expect(diagnosticsNote(0), endsWith('codes or network addresses.'));
  });

  testWidgets('only from a backend that records them', (tester) async {
    await _show(tester, null);
    expect(find.text('Advanced diagnostics'), findsNothing);
    expect(find.text('Record advanced diagnostics'), findsNothing);
  });

  testWidgets('its own section, off unless turned on', (tester) async {
    final harness = await _show(tester, _diagnostics(share: false));
    expect(find.text('Advanced diagnostics'), findsWidgets);
    expect(find.text('Record advanced diagnostics'), findsOneWidget);
    expect(find.text(diagnosticsNote(14)), findsOneWidget);
    expect(find.text('Delete all'), findsNothing);
    expect(find.text('Nothing received yet.'), findsNothing);
    await tester.tap(find.text('Record advanced diagnostics'));
    await pumpFrames(tester);
    final set = _lastSet(harness);
    expect((set.hasShare(), set.share, set.clear_2), (true, true, false));
  });

  testWidgets('on with nothing yet, and off again without asking', (
    tester,
  ) async {
    final harness = await _show(tester, _diagnostics());
    expect(find.text('Nothing received yet.'), findsOneWidget);
    await tester.tap(find.text('Record advanced diagnostics'));
    await pumpFrames(tester);
    expect(find.text('Turn off advanced diagnostics?'), findsNothing);
    final set = _lastSet(harness);
    expect((set.hasShare(), set.share), (true, false));
  });

  testWidgets("each source's part, deleted one at a time or all", (
    tester,
  ) async {
    final harness = await _show(
      tester,
      _diagnostics(
        sources: [
          _source('device:ipad', 'Studio iPad'),
          _source(
            'backend',
            '',
            bytes: 1200000,
            areas: [_area('connection', 1100000), _area('updates', 100000)],
          ),
        ],
      ),
    );
    expect(
      find.text('Studio iPad · Connections · 84.0 KB over the last 3 days'),
      findsOneWidget,
    );
    expect(
      find.text('This Claktory · 1.2 MB over the last 3 days'),
      findsOneWidget,
    );
    expect(find.text('Connections · 1.1 MB'), findsOneWidget);
    expect(find.text('Updates · 100 KB'), findsOneWidget);

    // One source's: at once, every area, sharing left as it is.
    await tester.tap(
      find.descendant(
        of: find.byType(DiagnosticsSourceRow).first,
        matching: find.text('Delete'),
      ),
    );
    await pumpFrames(tester);
    var set = _lastSet(harness);
    expect(
      (set.hasShare(), set.clear_2, set.sourceId, set.area),
      (false, true, 'device:ipad', ''),
    );

    // All of it: asked first; Cancel sends nothing.
    final before = _sets(harness).length;
    await tester.tap(find.text('Delete all'));
    await pumpFrames(tester);
    expect(find.text('Delete all diagnostics?'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(_sets(harness).length, before);
    await tester.tap(find.text('Delete all'));
    await pumpFrames(tester);
    await tester.tap(find.text('Delete').last);
    await pumpFrames(tester);
    set = _lastSet(harness);
    expect((set.hasShare(), set.clear_2, set.sourceId), (false, true, ''));
  });

  testWidgets('turning it off asks first when something is kept', (
    tester,
  ) async {
    final harness = await _show(
      tester,
      _diagnostics(sources: [_source('device:ipad', 'Studio iPad')]),
    );
    await tester.tap(find.text('Record advanced diagnostics'));
    await pumpFrames(tester);
    expect(find.text('Turn off advanced diagnostics?'), findsOneWidget);
    expect(
      find.text('This deletes everything recorded so far.'),
      findsOneWidget,
    );
    expect(_sets(harness), isEmpty);
    await tester.tap(find.text('Turn off'));
    await pumpFrames(tester);
    final set = _lastSet(harness);
    expect((set.hasShare(), set.share), (true, false));

    // The Workspace that follows shows it off and empty.
    await harness.apply(tester, _workspace(_diagnostics(share: false)));
    await pumpFrames(tester);
    expect(find.byType(DiagnosticsSourceRow), findsNothing);
  });

  testWidgets("on a phone's list, its glyph is one Claktory draws", (
    tester,
  ) async {
    final harness = Harness(_workspace(_diagnostics(share: false)));
    harness.page.value = AppPage.settings;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    final row = find.text('Advanced diagnostics');
    await tester.scrollUntilVisible(
      row,
      200,
      scrollable: find.byType(Scrollable).first,
    );
    await pumpFrames(tester);
    // Not the stand-in "?" for a glyph it doesn't know.
    expect(find.text('?'), findsNothing);
  });
}
