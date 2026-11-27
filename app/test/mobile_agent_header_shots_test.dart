// The phone's agent header (mobile/agent_header.dart),
// drawn to check by eye: with SHOTS set, PNGs of the header with a role
// and model, a long role that's cut, no role, no model, and marks under the
// name, on an iPhone SE and a Pro Max at the default text size and the
// largest:
//   SHOTS=/tmp/agent-header flutter test test/mobile_agent_header_shots_test.dart
// test-bundle: alone (loads the app's fonts)
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/mobile/agent_header.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

Future<void> _shot(WidgetTester tester, String name) async {
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

const _opus = 'anthropic/claude-opus-4-8';

final _agents = [
  Agent(
    id: 'bruno',
    name: 'Bruno',
    role: 'Project Manager',
    color: 'blue',
    modelAvailable: true,
    model: _opus,
  ),
  Agent(
    id: 'narancia',
    name: 'Narancia',
    role: 'Researcher for the iPhone, iPad and Android apps',
    color: 'orange',
    modelAvailable: true,
    model: _opus,
  ),
  Agent(
    id: 'rivet',
    name: 'Rivet',
    color: 'teal',
    modelAvailable: true,
    model: _opus,
  ),
  Agent(
    id: 'mista',
    name: 'Mista',
    role: 'Backend',
    color: 'red',
    model: _opus,
  ),
  Agent(
    id: 'polpo',
    name: 'Polpo',
    role: 'Efficiency',
    color: 'violet',
    modelAvailable: true,
    model: _opus,
    readsReports: true,
  ),
];

void main() {
  setUpAll(loadGeist);

  for (final (device, size, padding) in const [
    ('se', Size(375, 667), FakeViewPadding(top: 20)),
    ('promax', Size(440, 956), FakeViewPadding(top: 62, bottom: 34)),
  ]) {
    for (final scale in const [1.0, 3.1]) {
      testWidgets('$device at ${scale}x: role · model, a long role, no role, '
          'no model, marks', (tester) async {
        debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        tester.view.padding = padding;
        addTearDown(tester.view.resetPadding);
        final harness = Harness(
          Workspace(
            agents: _agents,
            models: [Model(id: _opus, name: 'Claude Opus 4.8')],
            settings: Settings(userName: 'Matt'),
          ),
        );
        await harness.show(
          tester,
          size: size,
          home: SafeArea(
            bottom: false,
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  for (final (i, agent) in _agents.indexed) ...[
                    if (i > 0) const SizedBox(height: 12),
                    PhoneAgentHeader(
                      agent: agent,
                      onBack: i.isEven ? () {} : null,
                      onMenu: () {},
                      onMore: () {},
                    ),
                  ],
                ],
              ),
            ),
          ),
        );
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
        await _shot(tester, '$device-$scale-headers');
      });
    }
  }
}
