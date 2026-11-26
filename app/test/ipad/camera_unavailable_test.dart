// An iPad's camera kept from the app while it shares the screen (Split
// View, Slide Over, Stage Manager beside another app) or while another app
// has it (Lucy's permissions §3 P7, Sorbet): the scanner's square says so
// over the black picture, and the words go when the camera comes back.
import 'package:factory_app/scanner.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart' show Harness;

Future<void> _native(String method, [Object? arguments]) =>
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .handlePlatformMessage(
          'claktory/scanner/7',
          const StandardMethodCodec().encodeMethodCall(
            MethodCall(method, arguments),
          ),
          (_) {},
        );

void main() {
  testWidgets('sharing the screen: the square says so, and the words go '
      'when the camera comes back; codes still read', (tester) async {
    final codes = <String>[];
    // What the screen around it hears.
    final told = <String?>[];
    await Harness(Workspace()).show(
      tester,
      home: NotificationListener<CameraAvailability>(
        onNotification: (notification) {
          told.add(notification.unavailable);
          return true;
        },
        child: Center(
          child: SizedBox.square(
            dimension: 320,
            child: CameraPreviewFor(
              id: 7,
              view: const ColoredBox(color: Color(0xFF000000)),
              onCode: codes.add,
            ),
          ),
        ),
      ),
    );
    Finder words() => find.byKey(const ValueKey('camera-unavailable'));
    expect(words(), findsNothing);

    await _native('unavailable', {'reason': 'multitasking'});
    await tester.pump();
    expect(
      find.text(
        "The camera isn't available in this window. Make Claktory full "
        'screen, or paste the code instead.',
      ),
      findsOneWidget,
    );

    await _native('unavailable', {'reason': 'inUse'});
    await tester.pump();
    expect(
      find.text(
        'Another app is using the camera. Close it, or paste the code '
        'instead.',
      ),
      findsOneWidget,
    );

    await _native('available');
    await tester.pump();
    expect(words(), findsNothing);

    // An interruption the person needn't act on says nothing.
    await _native('unavailable', {'reason': 'audio'});
    await tester.pump();
    expect(words(), findsNothing);

    // Told as it went, changed, and came back; not for the audio one.
    expect(told, [
      startsWith("The camera isn't available"),
      startsWith('Another app'),
      null,
    ]);

    await _native('scanned', 'claktory://pair/abc');
    expect(codes, ['claktory://pair/abc']);
  });
}
