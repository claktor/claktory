// Each device opens an app's view as its own surface (Bruno 28314): the
// device's id goes with the view's params, so what one device picks there
// isn't another's.
import 'dart:convert';

import 'package:factory_app/app_views/controller.dart';
import 'package:factory_app/app_views/transport.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_views_test.dart' show FakeAppViewTransport;

FakeAppViewTransport _transport() => FakeAppViewTransport()
  ..answer = (_) => AppSurfaceData(
    surfaceId: 'charts-1',
    appId: 'charts',
    title: 'Charts',
    state: AppViewState.open,
    snapshot: '',
    seq: 0,
    error: '',
  );

Future<String> _opened({required String params, String? device}) async {
  final transport = _transport();
  final controller = AppViewController(
    transport: transport,
    appId: 'charts',
    params: params,
    device: device,
  );
  await controller.open();
  controller.dispose();
  return transport.opens.last.params;
}

void main() {
  test("the device's id goes with the view's params", () async {
    expect(jsonDecode(await _opened(params: '', device: 'd-1')), {
      'device': 'd-1',
    });
    expect(
      jsonDecode(
        await _opened(params: '{"dashboard": "costs"}', device: 'd-2'),
      ),
      {'dashboard': 'costs', 'device': 'd-2'},
    );
  });

  test('without a device (a transport that knows none), the params are '
      'as they were', () async {
    expect(await _opened(params: '{"a": 1}'), '{"a": 1}');
    expect(await _opened(params: ''), '');
  });

  test('params that aren\'t an object go as they are, for the backend to '
      'refuse', () async {
    expect(await _opened(params: '[1]', device: 'd-1'), '[1]');
    expect(await _opened(params: '{', device: 'd-1'), '{');
  });
}
